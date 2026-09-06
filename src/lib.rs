#![doc = include_str!("../README.md")]

mod cancel;
mod channel;
mod error;
mod installer;
mod source;
mod store;
mod updater;
mod version;

#[cfg(feature = "clap")]
pub mod clap;

pub use cancel::CancellationToken;
pub use channel::{Channel, ChannelSpec, default_channels, lookup_channel, resolve};
pub use error::{Error, Result};
pub use installer::{Installer, ProcessStdio, ScriptInstaller};
pub use source::{GitHubSource, GitHubSourceBuilder, MAX_PER_PAGE, Release, Source};
pub use store::{FuncStore, MemStore, Store};
pub use updater::{CheckResult, UpdateOptions, Updater, UpdaterBuilder};
pub use version::{compare, normalize_version};
