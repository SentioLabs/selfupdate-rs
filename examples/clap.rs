//! Embed under a parent that already uses `-c` for configuration.
use clap::{Arg, Command};
use selfupdate::{CancellationToken, GitHubSource, MemStore, Updater, clap as commands};

fn main() -> selfupdate::Result<()> {
    let mut updater = Updater::builder("mytool", "dev", GitHubSource::new("acme", "mytool")?)
        .store(MemStore::default())
        .build();
    let matches = Command::new("mytool")
        .arg(Arg::new("config").long("config").short('c').global(true))
        .subcommand(commands::command(&updater))
        .get_matches();
    commands::dispatch(&mut updater, &matches, &CancellationToken::new())?;
    Ok(())
}
