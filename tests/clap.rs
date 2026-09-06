//! Optional Clap builders and dispatch.
#![cfg(feature = "clap")]
use ::clap::{Arg, Command, error::ErrorKind};
use selfupdate_rs::{clap as commands, *};
use std::{cell::RefCell, io, rc::Rc};

struct Fake;
impl Source for Fake {
    fn latest(&self, _: &CancellationToken) -> Result<Release> {
        Ok(Release {
            tag: "v2.0.0".into(),
            prerelease: false,
        })
    }
    fn list(&self, _: &CancellationToken, _: usize) -> Result<Vec<Release>> {
        Ok(vec![Release {
            tag: "v2.1.0-rc.1".into(),
            prerelease: true,
        }])
    }
}

#[test]
fn dispatch_check_show_and_switch_with_parent_config() {
    let mut out = vec![];
    let mut updater = Updater::builder("tool", "1.0.0", Fake)
        .store(MemStore::default())
        .output(&mut out)
        .build();
    let root = Command::new("tool")
        .arg(Arg::new("config").long("config").short('c').global(true))
        .subcommand(commands::command(&updater));
    root.clone().debug_assert();
    for argv in [
        vec!["tool", "self", "update", "--check", "-c", "settings.toml"],
        vec!["tool", "self", "channel"],
        vec!["tool", "self", "channel", "rc", "-y"],
        vec!["tool", "self", "update", "--check"],
    ] {
        let args = root.clone().try_get_matches_from(argv).unwrap();
        assert!(commands::dispatch(&mut updater, &args, &CancellationToken::new()).unwrap());
    }
    assert_eq!(updater.current_channel().unwrap(), Channel::RC);
    drop(updater);
    let out = String::from_utf8(out).unwrap();
    assert!(out.contains("Update available: v1.0.0 -> v2.0.0 (stable channel)"));
    assert!(out.contains("Current update channel: stable"));
    assert!(out.contains("v2.1.0-rc.1 (rc channel)"));
}

#[test]
fn help_short_flags_validation_and_custom_specs() {
    let updater = Updater::builder("tool", "dev", Fake).build();
    let error = commands::command(&updater)
        .try_get_matches_from(["self", "update", "--help"])
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::DisplayHelp);
    let help = error.to_string();
    assert!(help.contains("--check"));
    assert!(!help.contains("-c,"));
    assert!(help.contains("-f, --force"));
    assert!(help.contains("-y, --yes"));
    assert!(
        commands::command(&updater)
            .try_get_matches_from(["self", "update", "-c"])
            .is_err()
    );
    let args = commands::command_with_options(
        &updater,
        commands::CommandOptions {
            check_short: Some('c'),
        },
    )
    .try_get_matches_from(["self", "update", "-c"])
    .unwrap();
    assert!(args.subcommand_matches("update").unwrap().get_flag("check"));
    assert!(
        commands::command(&updater)
            .try_get_matches_from(["self", "channel", "beta"])
            .is_err()
    );
    assert!(
        commands::command(&updater)
            .try_get_matches_from(["self", "channel", "rc", "nightly"])
            .is_err()
    );
    assert!(
        commands::command(&updater)
            .try_get_matches_from(["self", "update", "extra"])
            .is_err()
    );
    let mut updater = Updater::builder("tool", "dev", Fake)
        .output(io::sink())
        .store(MemStore::default())
        .channels(vec![ChannelSpec {
            name: "beta".into(),
            pattern: None,
            warning: String::new(),
        }])
        .build();
    let error = commands::command(&updater)
        .try_get_matches_from(["self", "channel", "--help"])
        .unwrap_err();
    assert!(error.to_string().contains("beta"));
    assert!(!error.to_string().contains("nightly"));
    let args = commands::command(&updater)
        .try_get_matches_from(["self", "channel", "beta"])
        .unwrap();
    commands::dispatch(&mut updater, &args, &CancellationToken::new()).unwrap();
    assert_eq!(updater.current_channel().unwrap().as_str(), "beta");
}

#[test]
fn force_yes_dispatch_and_unrelated_commands() {
    struct Recording(Rc<RefCell<Vec<String>>>);
    impl Installer for Recording {
        fn install(&mut self, _: &CancellationToken, tag: &str) -> Result<()> {
            self.0.borrow_mut().push(tag.into());
            Ok(())
        }
    }
    let calls = Rc::new(RefCell::new(vec![]));
    let mut updater = Updater::builder("tool", "2.0.0", Fake)
        .output(io::sink())
        .installer(Recording(calls.clone()))
        .build();
    let args = commands::command(&updater)
        .try_get_matches_from(["self", "update", "-fy"])
        .unwrap();
    commands::dispatch(&mut updater, &args, &CancellationToken::new()).unwrap();
    assert_eq!(*calls.borrow(), ["v2.0.0"]);
    let args = Command::new("tool")
        .subcommand(Command::new("status"))
        .try_get_matches_from(["tool", "status"])
        .unwrap();
    assert!(!commands::dispatch(&mut updater, &args, &CancellationToken::new()).unwrap());
    let token = CancellationToken::new();
    token.cancel();
    let args = commands::command(&updater)
        .try_get_matches_from(["self", "update", "--check"])
        .unwrap();
    assert!(
        commands::dispatch(&mut updater, &args, &token)
            .unwrap_err()
            .is_cancelled()
    );
}
