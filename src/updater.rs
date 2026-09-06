use crate::{
    CancellationToken, Channel, ChannelSpec, Error, Installer, Result, Source, Store, compare,
    default_channels, lookup_channel, normalize_version, resolve,
};
use std::{
    cmp::Ordering,
    io::{self, BufRead, BufReader, Read, Write},
};

/// Controls update installation and confirmation.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct UpdateOptions {
    /// Install even when current is equal to or newer than latest.
    pub force: bool,
    /// Skip confirmation.
    pub yes: bool,
    /// Print status only; takes precedence over force and yes.
    pub check: bool,
}

/// The current version and selected release returned by `Updater::check`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CheckResult {
    /// Normalized running version.
    pub current: String,
    /// Original selected release tag.
    pub latest: String,
    /// Persisted channel, or stable when unset.
    pub channel: Channel,
    /// `Greater` means an update is available.
    pub cmp: Ordering,
}

type Hook<'a> = Box<dyn FnMut(&CancellationToken, &str, &str) -> Result<()> + 'a>;

/// Coordinates release lookup, confirmation, persistence, and installation.
/// Adapters and streams can be owned or borrowed for the updater's lifetime.
pub struct Updater<'a> {
    name: String,
    version: String,
    source: Box<dyn Source + 'a>,
    store: Option<Box<dyn Store + 'a>>,
    installer: Option<Box<dyn Installer + 'a>>,
    channels: Vec<ChannelSpec>,
    input: Box<dyn BufRead + 'a>,
    out: Box<dyn Write + 'a>,
    err_out: Box<dyn Write + 'a>,
    pre_install: Option<Hook<'a>>,
}

/// Builder requiring a source at construction. Store and installer are optional.
pub struct UpdaterBuilder<'a>(Updater<'a>);

impl<'a> Updater<'a> {
    /// Start configuration; streams default to the process's standard streams.
    pub fn builder(
        name: impl Into<String>,
        version: impl Into<String>,
        source: impl Source + 'a,
    ) -> UpdaterBuilder<'a> {
        UpdaterBuilder(Self {
            name: name.into(),
            version: version.into(),
            source: Box::new(source),
            store: None,
            installer: None,
            channels: default_channels(),
            input: Box::new(BufReader::new(io::stdin())),
            out: Box::new(io::stdout()),
            err_out: Box::new(io::stderr()),
            pre_install: None,
        })
    }
    /// Binary name used in status messages.
    pub fn name(&self) -> &str {
        &self.name
    }
    /// Configured channel specifications, in display order.
    pub fn channels(&self) -> &[ChannelSpec] {
        &self.channels
    }

    /// Read the persisted channel. Missing storage or an empty value means stable.
    pub fn current_channel(&self) -> Result<Channel> {
        let Some(store) = &self.store else {
            return Ok(Channel::STABLE);
        };
        let channel = store
            .channel()
            .map_err(|e| Error::ReadChannel(Box::new(e)))?;
        Ok(if channel.as_str().is_empty() {
            Channel::STABLE
        } else {
            channel
        })
    }

    /// Resolve the selected release without printing or installing.
    pub fn check(&self, token: &CancellationToken) -> Result<CheckResult> {
        token.check()?;
        let channel = self.current_channel()?;
        let latest = resolve(token, &*self.source, &channel, Some(&self.channels))?;
        token.check()?;
        Ok(CheckResult {
            current: normalize_version(&self.version),
            cmp: compare(&self.version, &latest),
            latest,
            channel,
        })
    }

    /// Check, print status, confirm, run the hook, then install.
    pub fn update(&mut self, token: &CancellationToken, options: UpdateOptions) -> Result<()> {
        let result = self.check(token).map_err(|e| Error::Check(Box::new(e)))?;
        if options.check || (!options.force && result.cmp != Ordering::Greater) {
            return match result.cmp {
                Ordering::Equal => self.output(token, &format!("{} {} ({}) is up to date\n", self.name, result.current, result.channel)),
                Ordering::Greater => self.output(token, &format!("Update available: {} -> {} ({} channel)\nRun '{} self update' to upgrade\n", result.current, result.latest, result.channel, self.name)),
                Ordering::Less => self.output(token, &format!("{} {} is newer than the latest {} release {}\n", self.name, result.current, result.channel, result.latest)),
            };
        }
        self.output(
            token,
            &format!(
                "Updating {} {} -> {}...\n",
                self.name, result.current, result.latest
            ),
        )?;
        if !options.yes && !self.confirm(token, "Continue? [y/N] ")? {
            return self.output(token, "Update cancelled.\n");
        }
        token.check()?;
        let installer = self.installer.as_mut().ok_or(Error::NoInstaller)?;
        if let Some(hook) = &mut self.pre_install {
            token.check()?;
            let outcome = hook(token, &result.current, &result.latest);
            token.check()?;
            outcome.map_err(|e| Error::PreInstall(Box::new(e)))?;
        }
        token.check()?;
        let outcome = installer.install(token, &result.latest);
        token.check()?;
        outcome
    }

    /// Validate, optionally warn and confirm, then persist a channel.
    pub fn switch_channel(
        &mut self,
        token: &CancellationToken,
        channel: impl Into<Channel>,
        yes: bool,
    ) -> Result<()> {
        token.check()?;
        let channel = channel.into();
        let spec =
            lookup_channel(&self.channels, &channel).ok_or_else(|| Error::InvalidChannel {
                channel: channel.to_string(),
                choices: self
                    .channels
                    .iter()
                    .map(|s| s.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
            })?;
        if self.store.is_none() {
            return Err(Error::NoStore);
        }
        if !spec.warning.is_empty() && !yes {
            let warning = format!("\n{}\n\n", spec.warning);
            token.check()?;
            self.err_out
                .write_all(warning.as_bytes())
                .map_err(Error::Output)?;
            self.err_out.flush().map_err(Error::Output)?;
            if !self.confirm(token, &format!("Switch to {channel} channel? [y/N] "))? {
                return self.output(token, "Cancelled.\n");
            }
        }
        token.check()?;
        self.store
            .as_mut()
            .ok_or(Error::NoStore)?
            .set_channel(channel.clone())
            .map_err(|e| Error::SaveChannel(Box::new(e)))?;
        self.output(token, &format!("Switched to {channel} channel\n"))?;
        if channel != Channel::STABLE {
            self.output(
                token,
                &format!(
                    "Run '{} self update' to get the latest {channel} build\n",
                    self.name
                ),
            )?;
        }
        Ok(())
    }

    /// Print the current channel to the configured output stream.
    pub fn show_channel(&mut self, token: &CancellationToken) -> Result<()> {
        token.check()?;
        let channel = self.current_channel()?;
        self.output(token, &format!("Current update channel: {channel}\n"))
    }

    fn output(&mut self, token: &CancellationToken, message: &str) -> Result<()> {
        token.check()?;
        self.out
            .write_all(message.as_bytes())
            .map_err(Error::Output)?;
        self.out.flush().map_err(Error::Output)?;
        token.check()
    }

    fn confirm(&mut self, token: &CancellationToken, prompt: &str) -> Result<bool> {
        self.output(token, prompt)?;
        let mut line = String::new();
        let read = self.input.read_line(&mut line);
        token.check()?;
        Ok(read.is_ok() && line.ends_with('\n') && matches!(line.trim(), "y" | "Y"))
    }
}

impl<'a> UpdaterBuilder<'a> {
    /// Configure persistent channel storage.
    pub fn store(mut self, store: impl Store + 'a) -> Self {
        self.0.store = Some(Box::new(store));
        self
    }
    /// Configure installation.
    pub fn installer(mut self, installer: impl Installer + 'a) -> Self {
        self.0.installer = Some(Box::new(installer));
        self
    }
    /// Replace the channel specifications; an empty vector means no channels.
    pub fn channels(mut self, channels: Vec<ChannelSpec>) -> Self {
        self.0.channels = channels;
        self
    }
    /// Supply confirmation input. Reads are buffered across prompts.
    pub fn input(mut self, input: impl Read + 'a) -> Self {
        self.0.input = Box::new(BufReader::new(input));
        self
    }
    /// Supply the status and prompt output stream.
    pub fn output(mut self, out: impl Write + 'a) -> Self {
        self.0.out = Box::new(out);
        self
    }
    /// Supply the channel-warning output stream.
    pub fn error_output(mut self, out: impl Write + 'a) -> Self {
        self.0.err_out = Box::new(out);
        self
    }
    /// Run a hook after confirmation and installer validation, before installation.
    pub fn pre_install(
        mut self,
        hook: impl FnMut(&CancellationToken, &str, &str) -> Result<()> + 'a,
    ) -> Self {
        self.0.pre_install = Some(Box::new(hook));
        self
    }
    /// Finish configuration.
    pub fn build(self) -> Updater<'a> {
        self.0
    }
}
