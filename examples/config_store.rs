//! A channel file adapted through closures; call `switch_channel` to save it.
use selfupdate_rs::{CancellationToken, Channel, Error, FuncStore, GitHubSource, Updater};
use std::{fs, io, path::PathBuf};

fn main() -> selfupdate_rs::Result<()> {
    let path = PathBuf::from("mytool-channel.txt");
    let read_path = path.clone();
    let store = FuncStore::new(
        move || match fs::read_to_string(&read_path) {
            Ok(value) => Ok(Channel::from(value.trim())),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Channel::STABLE),
            Err(e) => Err(Error::external(e)),
        },
        move |channel| fs::write(&path, channel.as_str()).map_err(Error::external),
    );
    let mut updater = Updater::builder("mytool", "dev", GitHubSource::new("acme", "mytool")?)
        .store(store)
        .build();
    updater.show_channel(&CancellationToken::new())
}
