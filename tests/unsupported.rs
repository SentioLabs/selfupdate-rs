//! Portable core exposes an explicit unsupported script-installation result.
#![cfg(not(any(target_os = "linux", target_os = "macos")))]
use selfupdate::{CancellationToken, Error, Installer, ScriptInstaller};

#[test]
fn unsupported_platform_is_explicit() {
    assert!(matches!(
        ScriptInstaller::new("https://example.com/install.sh")
            .install(&CancellationToken::new(), "v1.0.0"),
        Err(Error::UnsupportedPlatform)
    ));
}
