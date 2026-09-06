use std::cmp::Ordering;

/// Trim whitespace, add `v`, and map empty strings and `dev` to `v0.0.0-dev`.
pub fn normalize_version(version: &str) -> String {
    let version = version.trim();
    match version {
        "" | "dev" => "v0.0.0-dev".into(),
        v if v.starts_with('v') => v.into(),
        v => format!("v{v}"),
    }
}

/// Compare normalized versions using Go's `x/mod/semver` ordering.
///
/// `Greater` means **latest is newer**, so an update is available. Invalid
/// versions sort below valid versions and equal each other. Build metadata
/// does not affect precedence. `v1` and `v1.2` are valid shortened versions.
///
/// ```
/// use std::cmp::Ordering;
/// assert_eq!(selfupdate_rs::compare("1.0.0", "v2.0.0"), Ordering::Greater);
/// assert_eq!(selfupdate_rs::compare("1.0.0+one", "1.0.0+two"), Ordering::Equal);
/// ```
pub fn compare(current: &str, latest: &str) -> Ordering {
    compare_raw(&normalize_version(latest), &normalize_version(current))
}

struct Version<'a> {
    core: [&'a str; 3],
    pre: Option<&'a str>,
}

fn numeric(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}
fn number(s: &str) -> bool {
    numeric(s) && (s.len() == 1 || !s.starts_with('0'))
}

fn identifiers(s: &str, prerelease: bool) -> bool {
    s.split('.').all(|id| {
        !id.is_empty()
            && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            && (!prerelease || !numeric(id) || number(id))
    })
}

fn parse(s: &str) -> Option<Version<'_>> {
    let s = s.strip_prefix('v')?;
    let (s, build) = match s.split_once('+') {
        Some((s, build)) if identifiers(build, false) => (s, true),
        Some(_) => return None,
        None => (s, false),
    };
    let (s, pre) = match s.split_once('-') {
        Some((s, pre)) if identifiers(pre, true) => (s, Some(pre)),
        Some(_) => return None,
        None => (s, None),
    };
    let mut core = ["0"; 3];
    let mut count = 0;
    for (i, part) in s.split('.').enumerate() {
        if i >= 3 || !number(part) {
            return None;
        }
        core[i] = part;
        count += 1;
    }
    if count < 3 && (pre.is_some() || build) {
        return None;
    }
    Some(Version { core, pre })
}

fn compare_number(a: &str, b: &str) -> Ordering {
    a.len().cmp(&b.len()).then_with(|| a.cmp(b))
}

pub(crate) fn compare_raw(a: &str, b: &str) -> Ordering {
    let (a, b) = match (parse(a), parse(b)) {
        (Some(a), Some(b)) => (a, b),
        (Some(_), None) => return Ordering::Greater,
        (None, Some(_)) => return Ordering::Less,
        (None, None) => return Ordering::Equal,
    };
    for (a, b) in a.core.into_iter().zip(b.core) {
        let cmp = compare_number(a, b);
        if cmp != Ordering::Equal {
            return cmp;
        }
    }
    let (a, b) = match (a.pre, b.pre) {
        (Some(a), Some(b)) => (a, b),
        (None, Some(_)) => return Ordering::Greater,
        (Some(_), None) => return Ordering::Less,
        (None, None) => return Ordering::Equal,
    };
    let mut a = a.split('.');
    let mut b = b.split('.');
    loop {
        let cmp = match (a.next(), b.next()) {
            (Some(a), Some(b)) => match (numeric(a), numeric(b)) {
                (true, true) => compare_number(a, b),
                (true, false) => Ordering::Less,
                (false, true) => Ordering::Greater,
                (false, false) => a.cmp(b),
            },
            (Some(_), None) => return Ordering::Greater,
            (None, Some(_)) => return Ordering::Less,
            (None, None) => return Ordering::Equal,
        };
        if cmp != Ordering::Equal {
            return cmp;
        }
    }
}
