//! Cancellation from another thread; HTTP requests remain bounded by timeout.
use selfupdate_rs::{CancellationToken, GitHubSource, Updater};
use std::{thread, time::Duration};

fn main() -> selfupdate_rs::Result<()> {
    let token = CancellationToken::new();
    let cancel = token.clone();
    let worker = thread::spawn(move || {
        thread::sleep(Duration::from_millis(100));
        cancel.cancel();
    });
    let source = GitHubSource::builder("acme", "mytool")
        .timeout(Duration::from_secs(2))
        .build()?;
    let updater = Updater::builder("mytool", "dev", source).build();
    let result = updater.check(&token);
    worker.join().expect("cancellation thread");
    match result {
        Err(e) if e.is_cancelled() => Ok(()),
        other => other.map(|_| ()),
    }
}
