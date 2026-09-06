//! Basic check-only integration.
use selfupdate::{CancellationToken, GitHubSource, ScriptInstaller, UpdateOptions, Updater};

fn main() -> selfupdate::Result<()> {
    let mut updater = Updater::builder("mytool", "dev", GitHubSource::new("acme", "mytool")?)
        .installer(ScriptInstaller::new(
            "https://example.com/mytool/install.sh",
        ))
        .build();
    updater.update(
        &CancellationToken::new(),
        UpdateOptions {
            check: true,
            ..Default::default()
        },
    )
}
