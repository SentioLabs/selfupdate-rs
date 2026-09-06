//! A pre-install backup. This example only checks; use default options to install.
use selfupdate::{CancellationToken, Error, GitHubSource, ScriptInstaller, UpdateOptions, Updater};

fn main() -> selfupdate::Result<()> {
    let mut updater = Updater::builder("mytool", "1.0.0", GitHubSource::new("acme", "mytool")?)
        .installer(ScriptInstaller::new(
            "https://example.com/mytool/install.sh",
        ))
        .pre_install(|token, current, latest| {
            token.check()?;
            if current != latest {
                std::fs::copy("mytool.db", "mytool.db.backup").map_err(Error::external)?;
            }
            Ok(())
        })
        .build();
    updater.update(
        &CancellationToken::new(),
        UpdateOptions {
            check: true,
            ..Default::default()
        },
    )
}
