use crate::{CancellationToken, Error, MAX_PER_PAGE, Result, Source, version::compare_raw};
use regex::Regex;
use std::{borrow::Cow, cmp::Ordering, fmt};

/// A string-backed release channel; custom names are supported.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct Channel(Cow<'static, str>);

impl Channel {
    /// Official releases.
    pub const STABLE: Self = Self(Cow::Borrowed("stable"));
    /// Release candidates.
    pub const RC: Self = Self(Cow::Borrowed("rc"));
    /// Nightly builds.
    pub const NIGHTLY: Self = Self(Cow::Borrowed("nightly"));
    /// Return the channel name.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl Default for Channel {
    fn default() -> Self {
        Self::STABLE
    }
}
impl From<&str> for Channel {
    fn from(s: &str) -> Self {
        Self(Cow::Owned(s.into()))
    }
}
impl From<String> for Channel {
    fn from(s: String) -> Self {
        Self(Cow::Owned(s))
    }
}
impl fmt::Display for Channel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Channel lookup pattern and optional warning before switching.
#[derive(Clone, Debug)]
pub struct ChannelSpec {
    /// Name used by storage, output, and command-line choices.
    pub name: Channel,
    /// `None` uses the source's latest release; otherwise match listed tags.
    pub pattern: Option<Regex>,
    /// Empty means a switch requires no confirmation.
    pub warning: String,
}

/// The stable, rc, and nightly specifications inherited from go-selfupdate.
pub fn default_channels() -> Vec<ChannelSpec> {
    vec![
        ChannelSpec {
            name: Channel::STABLE,
            pattern: None,
            warning: String::new(),
        },
        ChannelSpec {
            name: Channel::RC,
            pattern: Some(
                Regex::new(r"^v[0-9]+\.[0-9]+\.[0-9]+-rc\.?[0-9]+\z").expect("built-in pattern"),
            ),
            warning: "Release candidates may contain bugs that have not been fully tested.".into(),
        },
        ChannelSpec {
            name: Channel::NIGHTLY,
            pattern: Some(
                Regex::new(r"^v[0-9]+\.[0-9]+\.[0-9]+-nightly\.[0-9]{8}\z")
                    .expect("built-in pattern"),
            ),
            warning: "Nightly builds come from the latest main branch and may be unstable.".into(),
        },
    ]
}

/// Find a configured channel by name.
pub fn lookup_channel<'a>(specs: &'a [ChannelSpec], channel: &Channel) -> Option<&'a ChannelSpec> {
    specs.iter().find(|spec| spec.name == *channel)
}

/// Resolve a channel from one newest-first page of at most 100 releases.
///
/// `None` selects the default specifications; an empty channel means stable.
/// Patterned channels fall back to `latest` whenever the page has no stable
/// release, even if the page has no matching channel tag.
pub fn resolve(
    token: &CancellationToken,
    source: &dyn Source,
    channel: &Channel,
    specs: Option<&[ChannelSpec]>,
) -> Result<String> {
    token.check()?;
    let defaults;
    let specs = match specs {
        Some(specs) => specs,
        None => {
            defaults = default_channels();
            &defaults
        }
    };
    let channel = if channel.as_str().is_empty() {
        &Channel::STABLE
    } else {
        channel
    };
    let spec =
        lookup_channel(specs, channel).ok_or_else(|| Error::UnknownChannel(channel.to_string()))?;
    let Some(pattern) = &spec.pattern else {
        let result = source.latest(token);
        token.check()?;
        return result.map(|r| r.tag);
    };
    let releases = source.list(token, MAX_PER_PAGE);
    token.check()?;
    let releases = releases?;
    let mut matched = None;
    let mut stable = None;
    for r in releases {
        if r.tag.is_empty() {
            continue;
        }
        if matched.is_none() && pattern.is_match(&r.tag) {
            matched = Some(r.tag.clone());
        }
        if stable.is_none() && !r.prerelease {
            stable = Some(r.tag);
        }
        if matched.is_some() && stable.is_some() {
            break;
        }
    }
    token.check()?;
    if stable.is_none() {
        let latest = source.latest(token);
        token.check()?;
        let latest = latest?;
        if !latest.prerelease && !latest.tag.is_empty() {
            stable = Some(latest.tag);
        }
    }
    match (matched, stable) {
        (Some(tag), Some(stable)) => Ok(if compare_raw(&stable, &tag) == Ordering::Greater {
            stable
        } else {
            tag
        }),
        (Some(tag), None) | (None, Some(tag)) => Ok(tag),
        (None, None) => Err(Error::NoRelease(channel.to_string())),
    }
}
