//! Ported Go workflows and compatibility regressions.
use regex::Regex;
use selfupdate::*;
use std::{
    cell::{Cell, RefCell},
    cmp::Ordering,
    io::{self, Cursor, Write},
    rc::Rc,
};

#[derive(Default)]
struct FakeSource {
    latest: Release,
    releases: Vec<Release>,
    calls: RefCell<Vec<String>>,
    fail_latest: bool,
    fail_list: bool,
    cancel_list: bool,
}
impl Source for FakeSource {
    fn latest(&self, _: &CancellationToken) -> Result<Release> {
        self.calls.borrow_mut().push("latest".into());
        if self.fail_latest {
            Err(Error::external(io::Error::other("offline")))
        } else {
            Ok(self.latest.clone())
        }
    }
    fn list(&self, token: &CancellationToken, limit: usize) -> Result<Vec<Release>> {
        self.calls.borrow_mut().push(format!("list:{limit}"));
        if self.cancel_list {
            token.cancel();
        }
        if self.fail_list {
            Err(Error::external(io::Error::other("offline")))
        } else {
            Ok(self.releases.clone())
        }
    }
}
fn rel(tag: &str, prerelease: bool) -> Release {
    Release {
        tag: tag.into(),
        prerelease,
    }
}
fn source(tag: &str) -> FakeSource {
    FakeSource {
        latest: rel(tag, false),
        ..Default::default()
    }
}
fn fixture() -> Vec<Release> {
    vec![
        rel("v0.11.0-rc3", true),
        rel("v0.11.0-rc2", true),
        rel("v0.11.0-rc.1", true),
        rel("v0.11.0-nightly.20260302", true),
        rel("v0.11.0-nightly.20260301", true),
        rel("v0.10.0", false),
    ]
}

#[test]
fn version_compatibility() {
    for (input, expected) in [
        ("", "v0.0.0-dev"),
        (" dev\n", "v0.0.0-dev"),
        ("0.1.0", "v0.1.0"),
        (" v1.2.3-rc.1 ", "v1.2.3-rc.1"),
        ("nonsense", "vnonsense"),
    ] {
        assert_eq!(normalize_version(input), expected);
    }
    for (current, latest, cmp) in [
        ("dev", "v1.0.0", Ordering::Greater),
        ("1.2.3", "v1.2.3", Ordering::Equal),
        ("v1.3.0-rc.1", "v1.3.0", Ordering::Greater),
        ("v2.0.0", "v1.0.0", Ordering::Less),
        ("v1.3.0-rc.1", "v1.3.0-rc.2", Ordering::Greater),
        ("v1.3.0", "v1.3.0-rc.9", Ordering::Less),
        ("v0.6.0", "v0.6.1-nightly.20260904", Ordering::Greater),
        ("v0.6.1-nightly.20260904", "v0.6.1", Ordering::Greater),
        ("1.0.0+abc", "1.0.0+def", Ordering::Equal),
        ("1.0.0+001", "1.0.0", Ordering::Equal),
        ("invalid", "bad", Ordering::Equal),
        ("invalid", "0.0.0", Ordering::Greater),
        ("1", "1.0.0", Ordering::Equal),
        ("1.2", "1.2.0", Ordering::Equal),
        ("1.0.0-rc9", "1.0.0-rc10", Ordering::Less),
        ("1.0.0-rc.9", "1.0.0-rc.10", Ordering::Greater),
        ("1.0.0-1", "1.0.0-a", Ordering::Greater),
        ("1.0.0-a", "1.0.0-a.1", Ordering::Greater),
        (
            "999999999999999999999999999999.0.0",
            "1000000000000000000000000000000",
            Ordering::Greater,
        ),
        (
            "1.0.0-99999999999999999999999999999",
            "1.0.0-100000000000000000000000000000",
            Ordering::Greater,
        ),
    ] {
        assert_eq!(compare(current, latest), cmp, "{current} -> {latest}");
        assert_eq!(compare(latest, current), cmp.reverse());
    }
    for invalid in [
        "01.0.0",
        "1.01.0",
        "1.0.01",
        "1.0.0-01",
        "1.0.0-",
        "1.0.0+",
        "1.0.0+x+y",
        "1.0.0-a..b",
        "1.0.0+bad_",
        "1.0.0-ä",
        "1.2-rc.1",
        "1+build",
        "1.2.3.4",
        "vv1.0.0",
        "V1.0.0",
        "1.０.0",
    ] {
        assert_eq!(compare(invalid, "0.0.0"), Ordering::Greater, "{invalid}");
        assert_eq!(compare(invalid, "garbage"), Ordering::Equal, "{invalid}");
    }
}

#[test]
fn default_patterns_and_custom_channels() {
    let specs = default_channels();
    assert_eq!(specs.len(), 3);
    assert!(specs[0].pattern.is_none());
    let rc = lookup_channel(&specs, &Channel::RC)
        .unwrap()
        .pattern
        .as_ref()
        .unwrap();
    for tag in ["v1.2.3-rc.1", "v1.2.3-rc1", "v0.16.0-rc9"] {
        assert!(rc.is_match(tag));
    }
    for tag in [
        "v1.2.3",
        "v1.2.3-nightly.20260903",
        "v1.2.3-rc",
        "v1.2.3-rc1\n",
        "v１.2.3-rc1",
    ] {
        assert!(!rc.is_match(tag));
    }
    let nightly = specs[2].pattern.as_ref().unwrap();
    assert!(nightly.is_match("v0.6.1-nightly.20260904"));
    assert!(!nightly.is_match("v0.6.1-nightly.2026"));
    assert!(lookup_channel(&specs, &Channel::from("beta")).is_none());
    let custom = [ChannelSpec {
        name: "beta".into(),
        pattern: Some(Regex::new(r"-beta\.").unwrap()),
        warning: String::new(),
    }];
    let source = FakeSource {
        releases: vec![rel("v2.0.0-beta.1", true), rel("v1.0.0", false)],
        ..Default::default()
    };
    assert_eq!(
        resolve(
            &CancellationToken::new(),
            &source,
            &"beta".into(),
            Some(&custom)
        )
        .unwrap(),
        "v2.0.0-beta.1"
    );
}

#[test]
fn resolution_preserves_newest_first_selection() {
    let token = CancellationToken::new();
    let src = FakeSource {
        latest: rel("v0.10.0", false),
        releases: fixture(),
        ..Default::default()
    };
    for ch in [Channel::STABLE, "".into()] {
        assert_eq!(resolve(&token, &src, &ch, None).unwrap(), "v0.10.0");
    }
    assert_eq!(*src.calls.borrow(), ["latest", "latest"]);
    assert_eq!(
        resolve(&token, &src, &Channel::RC, None).unwrap(),
        "v0.11.0-rc3"
    );
    assert_eq!(
        resolve(&token, &src, &Channel::NIGHTLY, None).unwrap(),
        "v0.11.0-nightly.20260302"
    );
    let src = FakeSource {
        releases: vec![
            rel("v2.0.0", false),
            rel("v1.0.0-rc1", true),
            rel("v9.0.0-rc1", true),
        ],
        ..Default::default()
    };
    assert_eq!(resolve(&token, &src, &Channel::RC, None).unwrap(), "v2.0.0");
    assert_eq!(*src.calls.borrow(), ["list:100"]);
}

#[test]
fn fallback_runs_with_and_without_matching_tags() {
    for matching in [true, false] {
        for (latest, expected) in [
            (
                rel("v0.19.0", false),
                if matching {
                    Some("v0.20.0-rc.101")
                } else {
                    Some("v0.19.0")
                },
            ),
            (rel("v0.21.0", false), Some("v0.21.0")),
            (
                rel("v0.21.0-rc.1", true),
                if matching {
                    Some("v0.20.0-rc.101")
                } else {
                    None
                },
            ),
            (
                rel("", false),
                if matching {
                    Some("v0.20.0-rc.101")
                } else {
                    None
                },
            ),
        ] {
            let releases = if matching {
                (2..=101)
                    .rev()
                    .map(|i| rel(&format!("v0.20.0-rc.{i}"), true))
                    .collect()
            } else {
                vec![rel("v0.20.0-nightly.20260903", true)]
            };
            let src = FakeSource {
                latest,
                releases,
                ..Default::default()
            };
            let result = resolve(&CancellationToken::new(), &src, &Channel::RC, None);
            match expected {
                Some(tag) => assert_eq!(result.unwrap(), tag),
                None => assert!(matches!(result, Err(Error::NoRelease(_)))),
            }
            assert_eq!(*src.calls.borrow(), ["list:100", "latest"]);
        }
    }
    let src = source("v1.0.0");
    assert_eq!(
        resolve(&CancellationToken::new(), &src, &Channel::RC, None).unwrap(),
        "v1.0.0"
    );
    let src = FakeSource {
        releases: vec![rel("v1.0.0", false)],
        ..Default::default()
    };
    assert_eq!(
        resolve(&CancellationToken::new(), &src, &Channel::NIGHTLY, None).unwrap(),
        "v1.0.0"
    );
    assert_eq!(*src.calls.borrow(), ["list:100"]);
}

#[test]
fn resolution_errors_and_cancellation_boundaries() {
    let token = CancellationToken::new();
    assert!(matches!(
        resolve(&token, &source(""), &"beta".into(), None),
        Err(Error::UnknownChannel(_))
    ));
    for src in [
        FakeSource {
            fail_list: true,
            ..Default::default()
        },
        FakeSource {
            fail_latest: true,
            ..Default::default()
        },
    ] {
        let error = resolve(&token, &src, &Channel::RC, None).unwrap_err();
        assert!(error.to_string().contains("offline"));
    }
    let src = FakeSource {
        cancel_list: true,
        ..Default::default()
    };
    assert!(
        resolve(&token, &src, &Channel::RC, None)
            .unwrap_err()
            .is_cancelled()
    );
    assert_eq!(*src.calls.borrow(), ["list:100"]);
    assert!(
        resolve(&token, &src, &Channel::STABLE, None)
            .unwrap_err()
            .is_cancelled()
    );
    assert_eq!(src.calls.borrow().len(), 1);
}

#[derive(Default)]
struct Recorder {
    events: Rc<RefCell<Vec<String>>>,
    fail: bool,
}
impl Installer for Recorder {
    fn install(&mut self, _: &CancellationToken, tag: &str) -> Result<()> {
        self.events.borrow_mut().push(format!("install:{tag}"));
        if self.fail {
            Err(Error::external(io::Error::other("install failed")))
        } else {
            Ok(())
        }
    }
}

#[test]
fn check_and_status_messages() {
    for check in [true, false] {
        for (current, latest, message) in [
            ("v1.0.0", "v1.0.0", "tool v1.0.0 (stable) is up to date\n"),
            (
                "v2.0.0",
                "v1.0.0",
                "tool v2.0.0 is newer than the latest stable release v1.0.0\n",
            ),
        ] {
            let mut output = vec![];
            let mut updater = Updater::builder("tool", current, source(latest))
                .output(&mut output)
                .build();
            updater
                .update(
                    &CancellationToken::new(),
                    UpdateOptions {
                        check,
                        ..Default::default()
                    },
                )
                .unwrap();
            drop(updater);
            assert_eq!(String::from_utf8(output).unwrap(), message);
        }
    }
    let mut output = vec![];
    let mut updater = Updater::builder("tool", "1.0.0", source("v2.0.0"))
        .output(&mut output)
        .build();
    updater
        .update(
            &CancellationToken::new(),
            UpdateOptions {
                check: true,
                force: true,
                yes: true,
            },
        )
        .unwrap();
    drop(updater);
    assert_eq!(
        String::from_utf8(output).unwrap(),
        "Update available: v1.0.0 -> v2.0.0 (stable channel)\nRun 'tool self update' to upgrade\n"
    );
    let updater = Updater::builder(
        "tool",
        "0.10.0",
        FakeSource {
            releases: fixture(),
            ..Default::default()
        },
    )
    .store(MemStore {
        current: Channel::RC,
    })
    .build();
    assert_eq!(
        updater.check(&CancellationToken::new()).unwrap(),
        CheckResult {
            current: "v0.10.0".into(),
            latest: "v0.11.0-rc3".into(),
            channel: Channel::RC,
            cmp: Ordering::Greater,
        }
    );
}

#[test]
fn only_y_or_uppercase_y_accepts_and_eof_declines() {
    for input in ["y\n", "Y\n", " y \r\n", "yes\n", "n\n", "\n", "", "y", "Y"] {
        let events = Rc::new(RefCell::new(vec![]));
        let mut output = vec![];
        let mut updater = Updater::builder("tool", "v1.0.0", source("v2.0.0"))
            .installer(Recorder {
                events: events.clone(),
                fail: false,
            })
            .input(Cursor::new(input))
            .output(&mut output)
            .build();
        updater
            .update(&CancellationToken::new(), UpdateOptions::default())
            .unwrap();
        drop(updater);
        let accepted = input.ends_with('\n') && matches!(input.trim(), "y" | "Y");
        assert_eq!(events.borrow().len(), usize::from(accepted), "{input:?}");
        assert_eq!(
            String::from_utf8(output).unwrap(),
            format!(
                "Updating tool v1.0.0 -> v2.0.0...\nContinue? [y/N] {}",
                if accepted { "" } else { "Update cancelled.\n" }
            )
        );
    }
}

#[test]
fn force_hook_order_and_installer_failure() {
    for latest in ["v1.0.0", "v0.9.0"] {
        let events = Rc::new(RefCell::new(vec![]));
        let hook_events = events.clone();
        let mut updater = Updater::builder("tool", "1.0.0", source(latest))
            .output(io::sink())
            .installer(Recorder {
                events: events.clone(),
                fail: false,
            })
            .pre_install(move |_, current, latest| {
                hook_events
                    .borrow_mut()
                    .push(format!("pre:{current}->{latest}"));
                Ok(())
            })
            .build();
        updater
            .update(
                &CancellationToken::new(),
                UpdateOptions {
                    force: true,
                    yes: true,
                    check: false,
                },
            )
            .unwrap();
        assert_eq!(
            *events.borrow(),
            [format!("pre:v1.0.0->{latest}"), format!("install:{latest}")]
        );
    }
    let mut updater = Updater::builder("tool", "dev", source("v1.0.0"))
        .output(io::sink())
        .installer(Recorder {
            fail: true,
            ..Default::default()
        })
        .build();
    assert_eq!(
        updater
            .update(
                &CancellationToken::new(),
                UpdateOptions {
                    yes: true,
                    ..Default::default()
                }
            )
            .unwrap_err()
            .to_string(),
        "install failed"
    );
}

#[test]
fn hook_and_missing_installer_prevent_installation() {
    let mut installer = Recorder::default();
    let mut updater = Updater::builder("tool", "dev", source("v1.0.0"))
        .output(io::sink())
        .installer(&mut installer)
        .pre_install(|_, _, _| Err(Error::external(io::Error::other("backup failed"))))
        .build();
    let error = updater
        .update(
            &CancellationToken::new(),
            UpdateOptions {
                yes: true,
                ..Default::default()
            },
        )
        .unwrap_err();
    assert!(matches!(error, Error::PreInstall(_)));
    assert!(std::error::Error::source(&error).is_some());
    drop(updater);
    assert!(installer.events.borrow().is_empty());
    let mut updater = Updater::builder("tool", "dev", source("v1.0.0"))
        .output(io::sink())
        .pre_install(|_, _, _| panic!("no installer must skip hook"))
        .build();
    assert!(matches!(
        updater.update(
            &CancellationToken::new(),
            UpdateOptions {
                yes: true,
                ..Default::default()
            }
        ),
        Err(Error::NoInstaller)
    ));
    let mut updater = Updater::builder(
        "tool",
        "dev",
        FakeSource {
            fail_latest: true,
            ..Default::default()
        },
    )
    .build();
    assert!(matches!(
        updater.update(&CancellationToken::new(), UpdateOptions::default()),
        Err(Error::Check(_))
    ));
}

#[test]
fn cancellation_in_hook_and_prompt_prevents_installation() {
    let mut installer = Recorder::default();
    let token = CancellationToken::new();
    let mut updater = Updater::builder("tool", "dev", source("v1.0.0"))
        .output(io::sink())
        .installer(&mut installer)
        .pre_install(|token, _, _| {
            token.cancel();
            Ok(())
        })
        .build();
    assert!(
        updater
            .update(
                &token,
                UpdateOptions {
                    yes: true,
                    ..Default::default()
                }
            )
            .unwrap_err()
            .is_cancelled()
    );
    drop(updater);
    assert!(installer.events.borrow().is_empty());
    struct CancelRead(CancellationToken);
    impl io::Read for CancelRead {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            self.0.cancel();
            buffer[..2].copy_from_slice(b"y\n");
            Ok(2)
        }
    }
    let token = CancellationToken::new();
    let mut updater = Updater::builder("tool", "dev", source("v1.0.0"))
        .output(io::sink())
        .installer(&mut installer)
        .input(CancelRead(token.clone()))
        .build();
    assert!(
        updater
            .update(&token, UpdateOptions::default())
            .unwrap_err()
            .is_cancelled()
    );
    drop(updater);
    assert!(installer.events.borrow().is_empty());
}

#[test]
fn stores_and_channel_switch_workflows() {
    let mut store = MemStore::default();
    let mut output = vec![];
    let mut warnings = vec![];
    let mut updater = Updater::builder("tool", "dev", source("v1.0.0"))
        .store(&mut store)
        .input(Cursor::new("y\nn\ny\n"))
        .output(&mut output)
        .error_output(&mut warnings)
        .build();
    let token = CancellationToken::new();
    assert_eq!(updater.current_channel().unwrap(), Channel::STABLE);
    updater.switch_channel(&token, Channel::RC, false).unwrap();
    assert_eq!(updater.current_channel().unwrap(), Channel::RC);
    updater
        .switch_channel(&token, Channel::NIGHTLY, false)
        .unwrap();
    assert_eq!(updater.current_channel().unwrap(), Channel::RC);
    // Buffering must retain the third line across calls.
    updater
        .switch_channel(&token, Channel::NIGHTLY, false)
        .unwrap();
    assert_eq!(updater.current_channel().unwrap(), Channel::NIGHTLY);
    updater
        .switch_channel(&token, Channel::STABLE, false)
        .unwrap();
    updater
        .switch_channel(&token, Channel::NIGHTLY, true)
        .unwrap();
    drop(updater);
    assert_eq!(store.current, Channel::NIGHTLY);
    let output = String::from_utf8(output).unwrap();
    assert!(
        output.contains(
            "Switched to rc channel\nRun 'tool self update' to get the latest rc build\n"
        )
    );
    assert!(output.contains("Cancelled.\n"));
    assert!(output.contains("Switched to stable channel\nSwitched to nightly channel"));
    let warnings = String::from_utf8(warnings).unwrap();
    assert_eq!(warnings.matches("Nightly builds").count(), 2);
    assert!(
        warnings.starts_with(
            "\nRelease candidates may contain bugs that have not been fully tested.\n\n"
        )
    );
}

#[test]
fn missing_and_failing_stores() {
    let token = CancellationToken::new();
    let mut updater = Updater::builder("tool", "dev", source("")).build();
    assert_eq!(updater.current_channel().unwrap(), Channel::STABLE);
    assert!(matches!(
        updater.switch_channel(&token, Channel::RC, true),
        Err(Error::NoStore)
    ));
    assert_eq!(
        updater
            .switch_channel(&token, "beta", true)
            .unwrap_err()
            .to_string(),
        "invalid channel \"beta\": must be one of stable, rc, nightly"
    );
    let mut store = FuncStore::default();
    assert!(matches!(store.channel(), Err(Error::NoStore)));
    assert!(matches!(
        store.set_channel(Channel::RC),
        Err(Error::NoStore)
    ));
    let updater = Updater::builder("tool", "dev", source(""))
        .store(FuncStore::new(|| Ok("".into()), |_| Ok(())))
        .build();
    assert_eq!(updater.current_channel().unwrap(), Channel::STABLE);
    let updater = Updater::builder("tool", "dev", source(""))
        .store(FuncStore::default())
        .build();
    assert!(matches!(
        updater.current_channel(),
        Err(Error::ReadChannel(_))
    ));
    let mut updater = Updater::builder("tool", "dev", source(""))
        .store(FuncStore::new(
            || Ok(Channel::STABLE),
            |_| Err(Error::external(io::Error::other("disk full"))),
        ))
        .build();
    assert!(matches!(
        updater.switch_channel(&token, Channel::RC, true),
        Err(Error::SaveChannel(_))
    ));
    let mut updater = Updater::builder("tool", "dev", source(""))
        .channels(vec![])
        .build();
    assert!(matches!(
        updater.switch_channel(&token, Channel::STABLE, true),
        Err(Error::InvalidChannel { .. })
    ));
}

struct BrokenOutput {
    flush_only: bool,
}
impl Write for BrokenOutput {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.flush_only {
            Ok(bytes.len())
        } else {
            Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed"))
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed"))
    }
}

#[test]
fn output_and_flush_errors_propagate_before_installation_or_save() {
    for flush_only in [false, true] {
        for check in [true, false] {
            let mut updater = Updater::builder("tool", "dev", source("v1.0.0"))
                .output(BrokenOutput { flush_only })
                .pre_install(|_, _, _| panic!("must not run"))
                .build();
            assert!(matches!(
                updater.update(
                    &CancellationToken::new(),
                    UpdateOptions {
                        check,
                        yes: true,
                        force: false
                    }
                ),
                Err(Error::Output(_))
            ));
        }
        let saved = Cell::new(false);
        let mut updater = Updater::builder("tool", "dev", source(""))
            .store(FuncStore::new(
                || Ok(Channel::STABLE),
                |_| {
                    saved.set(true);
                    Ok(())
                },
            ))
            .output(io::sink())
            .error_output(BrokenOutput { flush_only })
            .build();
        assert!(matches!(
            updater.switch_channel(&CancellationToken::new(), Channel::RC, false),
            Err(Error::Output(_))
        ));
        assert!(!saved.get());
    }
    let mut updater = Updater::builder("tool", "dev", source(""))
        .store(MemStore::default())
        .output(BrokenOutput { flush_only: false })
        .build();
    assert!(matches!(
        updater.switch_channel(&CancellationToken::new(), Channel::RC, true),
        Err(Error::Output(_))
    ));
    assert_eq!(updater.current_channel().unwrap(), Channel::RC);
}

#[test]
fn traits_are_object_safe() {
    let _: Box<dyn Source> = Box::new(FakeSource::default());
    let _: Box<dyn Store> = Box::new(MemStore::default());
    let _: Box<dyn Installer> = Box::new(Recorder::default());
}

#[test]
fn consumer_error_type_survives_workflow_context() {
    let mut updater = Updater::builder(
        "tool",
        "dev",
        FakeSource {
            fail_latest: true,
            ..Default::default()
        },
    )
    .build();
    let error = updater
        .update(&CancellationToken::new(), UpdateOptions::default())
        .unwrap_err();
    let mut cause: &(dyn std::error::Error + 'static) = &error;
    while let Some(source) = cause.source() {
        cause = source;
    }
    assert_eq!(
        cause.downcast_ref::<io::Error>().unwrap().to_string(),
        "offline"
    );
}

#[test]
fn input_errors_decline_without_running_hook_or_installer() {
    struct BrokenInput;
    impl io::Read for BrokenInput {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("closed"))
        }
    }
    let mut out = vec![];
    let mut updater = Updater::builder("tool", "dev", source("v1.0.0"))
        .input(BrokenInput)
        .output(&mut out)
        .pre_install(|_, _, _| panic!("declined updates must skip the hook"))
        .build();
    updater
        .update(&CancellationToken::new(), UpdateOptions::default())
        .unwrap();
    drop(updater);
    assert!(
        String::from_utf8(out)
            .unwrap()
            .ends_with("Update cancelled.\n")
    );
}

#[test]
fn cancellation_before_persistence_does_not_save() {
    struct CancelOutput(CancellationToken);
    impl Write for CancelOutput {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.cancel();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let token = CancellationToken::new();
    let saved = Cell::new(false);
    let mut updater = Updater::builder("tool", "dev", source(""))
        .input(Cursor::new("y\n"))
        .output(CancelOutput(token.clone()))
        .error_output(io::sink())
        .store(FuncStore::new(
            || Ok(Channel::STABLE),
            |_| {
                saved.set(true);
                Ok(())
            },
        ))
        .build();
    assert!(
        updater
            .switch_channel(&token, Channel::RC, false)
            .unwrap_err()
            .is_cancelled()
    );
    assert!(!saved.get());
}
