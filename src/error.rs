use std::{io, process::ExitStatus};

/// Result returned by self-update operations and consumer adapters.
pub type Result<T> = std::result::Result<T, Error>;

/// Failures preserve their underlying causes through `std::error::Error::source`.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// Cooperative cancellation was requested.
    #[error("selfupdate: cancelled")]
    Cancelled,
    /// An installation was requested without an installer.
    #[error("selfupdate: no installer configured")]
    NoInstaller,
    /// Channel persistence is unavailable.
    #[error("selfupdate: channel store not configured")]
    NoStore,
    /// The requested channel has no specification.
    #[error("unknown channel {0:?}")]
    UnknownChannel(String),
    /// A channel switch was rejected; includes the configured choices.
    #[error("invalid channel {channel:?}: must be one of {choices}")]
    InvalidChannel {
        /// Requested name.
        channel: String,
        /// Comma-separated configured names.
        choices: String,
    },
    /// No release was available for the channel.
    #[error("no {0} release found")]
    NoRelease(String),
    /// A setting is unusable.
    #[error("selfupdate: {0}")]
    Configuration(String),
    /// Update lookup failed.
    #[error("check for updates: {0}")]
    Check(#[source] Box<Error>),
    /// Reading persisted configuration failed.
    #[error("read update channel: {0}")]
    ReadChannel(#[source] Box<Error>),
    /// Persisting configuration failed.
    #[error("save update channel: {0}")]
    SaveChannel(#[source] Box<Error>),
    /// A pre-install hook failed.
    #[error("pre-install step failed: {0}")]
    PreInstall(#[source] Box<Error>),
    /// Writing or flushing a user-facing stream failed.
    #[error("selfupdate: output failed: {0}")]
    Output(#[source] io::Error),
    /// HTTP request construction, transport, or body reading failed.
    #[error("github: {0}")]
    Http(#[source] reqwest::Error),
    /// GitHub returned a status other than 200.
    #[error("github: {path} returned status {status}")]
    HttpStatus {
        /// Request path without credentials.
        path: String,
        /// HTTP response status.
        status: u16,
    },
    /// GitHub returned an invalid JSON payload.
    #[error("github: decode {path}: {source}")]
    Decode {
        /// Request path.
        path: String,
        /// JSON decoding failure.
        #[source]
        source: serde_json::Error,
    },
    /// A tag fails the script installer's validation.
    #[error("selfupdate: refusing to install unsafe tag {0:?}")]
    UnsafeTag(String),
    /// The script installer supports macOS and Linux only.
    #[error("selfupdate: script installation is unsupported on this platform")]
    UnsupportedPlatform,
    /// Spawning, monitoring, or stopping the install process failed.
    #[error("install script failed: {0}")]
    Process(#[source] io::Error),
    /// The install pipeline exited unsuccessfully.
    #[error("install script failed: {0}")]
    InstallExit(ExitStatus),
    /// A consumer-provided implementation failed.
    #[error("{0}")]
    External(#[from] Box<dyn std::error::Error + Send + Sync>),
}

impl Error {
    /// Wrap a consumer error while preserving its concrete type and cause.
    pub fn external(error: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::External(Box::new(error))
    }

    /// Whether this error, including workflow wrappers, represents cancellation.
    pub fn is_cancelled(&self) -> bool {
        match self {
            Self::Cancelled => true,
            Self::Check(e) | Self::ReadChannel(e) | Self::SaveChannel(e) | Self::PreInstall(e) => {
                e.is_cancelled()
            }
            _ => false,
        }
    }
}
