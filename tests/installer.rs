//! Harmless local script and real process-group tests; no binary replacement.
#![cfg(any(target_os = "linux", target_os = "macos"))]
mod common;
use common::{Reply, Server, wait_until};
use nix::{
    errno::Errno,
    pty::openpty,
    sys::signal::{Signal, kill, killpg},
    unistd::{Pid, getpgrp},
};
use selfupdate_rs::*;
use std::{
    fs::{self, File},
    io::{Read, Write},
    os::unix::process::CommandExt,
    path::Path,
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

fn file_url(path: &Path) -> String {
    reqwest::Url::from_file_path(path).unwrap().into()
}
fn quiet(url: impl Into<String>) -> ScriptInstaller {
    ScriptInstaller::new(url)
        .stdin(ProcessStdio::Null)
        .stdout(ProcessStdio::Null)
        .stderr(ProcessStdio::Null)
}
fn script(dir: &Path, body: &str) -> String {
    let path = dir.join("install.sh");
    fs::write(&path, body).unwrap();
    file_url(&path)
}
fn quoted(path: &Path) -> String {
    format!("'{}'", path.to_str().unwrap().replace('\'', "'\\''"))
}

#[test]
fn exact_arguments_and_reusable_file_stdio() {
    let dir = tempfile::tempdir().unwrap();
    let url = script(
        dir.path(),
        "printf '%s\\n' \"$#\" \"$1\" \"$2\"; printf 'warning\\n' >&2\n",
    );
    let out = dir.path().join("out");
    let err = dir.path().join("err");
    let mut installer = quiet(url)
        .stdout(File::create(&out).unwrap())
        .stderr(File::create(&err).unwrap());
    installer
        .install(&CancellationToken::new(), "v9.9.9-rc.1+build")
        .unwrap();
    installer
        .install(&CancellationToken::new(), "1.2.3")
        .unwrap();
    assert_eq!(
        fs::read_to_string(out).unwrap(),
        "2\n--force\n--tag=v9.9.9-rc.1+build\n2\n--force\n--tag=1.2.3\n"
    );
    assert_eq!(fs::read_to_string(err).unwrap(), "warning\nwarning\n");
}

#[test]
fn url_metacharacters_are_literal_data() {
    let server = Server::new(vec![Reply::ok("printf 'installed\\n'\n")]);
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out");
    let url = format!("{}/install.sh?x='quoted'&y=$(id);echo", server.url);
    quiet(url)
        .stdout(File::create(&out).unwrap())
        .install(&CancellationToken::new(), "v1.0.0")
        .unwrap();
    assert_eq!(fs::read_to_string(out).unwrap(), "installed\n");
    assert!(server.requests()[0].starts_with("GET /install.sh?x='quoted'&y=$(id);echo HTTP/1.1"));

    let marker = dir.path().join("injected");
    let missing = file_url(&dir.path().join("missing"));
    for url in [
        format!("{missing};touch${{IFS}}{}", marker.display()),
        format!("{missing}$(touch${{IFS}}{})", marker.display()),
        format!("{missing}`touch${{IFS}}{}`", marker.display()),
        format!("{missing}' ; touch {} ; #", marker.display()),
        "--help".into(),
    ] {
        assert!(
            quiet(url)
                .install(&CancellationToken::new(), "v1.0.0")
                .is_err()
        );
        assert!(!marker.exists());
    }
}

#[test]
fn invalid_tags_empty_url_and_pre_cancelled_install_never_execute() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("ran");
    let url = script(dir.path(), &format!("touch {}\n", quoted(&marker)));
    for tag in [
        "",
        "v1; rm -rf /",
        "v1 $(x)",
        "v1`x`",
        "v1&&x",
        "v1|x",
        "v1 x",
        "../x",
        "-v1",
        "v1\n",
        "é1",
    ] {
        assert!(matches!(
            quiet(&url).install(&CancellationToken::new(), tag),
            Err(Error::UnsafeTag(_))
        ));
    }
    assert!(matches!(
        quiet("").install(&CancellationToken::new(), "v1"),
        Err(Error::Configuration(_))
    ));
    let token = CancellationToken::new();
    token.cancel();
    assert!(
        quiet(url)
            .install(&token, "v1.0.0")
            .unwrap_err()
            .is_cancelled()
    );
    assert!(!marker.exists());
}

#[test]
fn curl_failures_and_nonzero_script_status_fail_the_pipeline() {
    let dir = tempfile::tempdir().unwrap();
    let error = quiet(file_url(&dir.path().join("missing.sh")))
        .install(&CancellationToken::new(), "v1.0.0")
        .unwrap_err();
    assert!(matches!(error, Error::InstallExit(status) if !status.success()));
    let url = script(dir.path(), "exit 3\n");
    let error = quiet(url)
        .install(&CancellationToken::new(), "v1.0.0")
        .unwrap_err();
    assert!(matches!(error, Error::InstallExit(status) if status.code() == Some(3)));
}

fn running(pid: i32) -> bool {
    if kill(Pid::from_raw(pid), None) == Err(Errno::ESRCH) {
        return false;
    }
    let output = Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .unwrap();
    let state = String::from_utf8_lossy(&output.stdout);
    !state.trim().is_empty() && !state.trim().starts_with('Z')
}

// A test panic must not leave a pipeline behind. Only groups positively known
// to be separate from the caller are eligible for cleanup.
struct GroupGuard(i32);
impl Drop for GroupGuard {
    fn drop(&mut self) {
        if self.0 > 0 && self.0 != getpgrp().as_raw() {
            let _ = killpg(Pid::from_raw(self.0), Signal::SIGKILL);
        }
    }
}

#[test]
fn cancellation_stops_descendants_and_reaps_pipeline_leader() {
    let dir = tempfile::tempdir().unwrap();
    let ready = dir.path().join("ready");
    let url = script(
        dir.path(),
        &format!(
            "sleep 31.415 &\nchild=$!\nprintf '%s\\n%s\\n%s\\n' \"$$\" \"$child\" \"$PPID\" > {}\nwait\n",
            quoted(&ready)
        ),
    );
    let token = CancellationToken::new();
    let cancel = token.clone();
    let worker = thread::spawn(move || quiet(url).install(&token, "v1.0.0"));
    wait_until(
        || fs::read_to_string(&ready).is_ok_and(|s| s.lines().count() == 3),
        Duration::from_secs(4),
    );
    let pids: Vec<i32> = fs::read_to_string(ready)
        .unwrap()
        .lines()
        .map(|s| s.parse().unwrap())
        .collect();
    let guard = GroupGuard(pids[2]);
    assert_ne!(guard.0, getpgrp().as_raw());
    let start = Instant::now();
    cancel.cancel();
    assert!(worker.join().unwrap().unwrap_err().is_cancelled());
    assert!(start.elapsed() < Duration::from_secs(8));
    wait_until(
        || pids.iter().all(|&pid| !running(pid)),
        Duration::from_secs(2),
    );
    // The direct child must have been reaped, not merely left as a zombie.
    assert_eq!(kill(Pid::from_raw(pids[2]), None), Err(Errno::ESRCH));
}

#[test]
fn cancellation_escalates_for_descendant_after_leader_exits() {
    let dir = tempfile::tempdir().unwrap();
    let ready = dir.path().join("ready");
    let child_ready = dir.path().join("child-ready");
    let child_script = dir.path().join("child.sh");
    fs::write(
        &child_script,
        format!(
            "trap '' TERM\nprintf '%s\\n' \"$$\" > {}\nwhile :; do sleep 0.1; done\n",
            quoted(&child_ready)
        ),
    )
    .unwrap();
    let url = script(
        dir.path(),
        &format!(
            "trap 'exit 0' TERM\nbash {} &\nprintf '%s\\n%s\\n' \"$$\" \"$PPID\" > {}\nwait\n",
            quoted(&child_script),
            quoted(&ready)
        ),
    );
    let token = CancellationToken::new();
    let cancel = token.clone();
    let worker = thread::spawn(move || quiet(url).install(&token, "v1.0.0"));
    wait_until(
        || {
            fs::read_to_string(&child_ready).is_ok_and(|s| !s.trim().is_empty())
                && fs::read_to_string(&ready).is_ok_and(|s| s.lines().count() == 2)
        },
        Duration::from_secs(4),
    );
    let pids: Vec<i32> = fs::read_to_string(ready)
        .unwrap()
        .lines()
        .map(|s| s.parse().unwrap())
        .collect();
    let child: i32 = fs::read_to_string(child_ready)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let _guard = GroupGuard(pids[1]);
    let start = Instant::now();
    cancel.cancel();
    wait_until(|| !running(pids[1]), Duration::from_secs(3));
    assert!(
        running(child),
        "SIGTERM-ignoring descendant must outlive leader"
    );
    assert!(worker.join().unwrap().unwrap_err().is_cancelled());
    assert!(start.elapsed() >= Duration::from_secs(5));
    assert!(start.elapsed() < Duration::from_secs(9));
    wait_until(|| !running(child), Duration::from_secs(2));
}

#[test]
fn terminal_pipeline_shares_foreground_group_reads_tty_and_receives_ctrl_c() {
    let dir = tempfile::tempdir().unwrap();
    let group_file = dir.path().join("group");
    let url = script(
        dir.path(),
        &format!(
            "ps -o pgid= -p $$ > {}\nprintf 'PTY-READY\\n'\nIFS= read -r answer < /dev/tty\nprintf 'ANSWER:%s\\n' \"$answer\"\ntrap 'printf \"PTY-INTERRUPTED\\n\"; exit 17' INT\nprintf 'PTY-WAITING\\n'\nwhile :; do sleep 0.1; done\n",
            quoted(&group_file)
        ),
    );
    let pty = openpty(None, None).unwrap();
    let mut master = File::from(pty.master);
    let slave = File::from(pty.slave);
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "pty_helper", "--nocapture"])
        .env("SELFUPDATE_TEST_PTY_URL", url)
        .stdin(Stdio::from(slave.try_clone().unwrap()))
        .stdout(Stdio::from(slave.try_clone().unwrap()))
        .stderr(Stdio::from(slave));
    // SAFETY: only async-signal-safe syscalls run between fork and exec.
    unsafe {
        command.pre_exec(|| {
            if nix::libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            if nix::libc::ioctl(0, nix::libc::TIOCSCTTY as _, 0) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn().unwrap();
    drop(command);
    let _guard = GroupGuard(child.id() as i32);
    let output = Arc::new(Mutex::new(String::new()));
    let captured = output.clone();
    let mut reader = master.try_clone().unwrap();
    let read_thread = thread::spawn(move || {
        let mut buffer = [0u8; 1024];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(n) => captured
                    .lock()
                    .unwrap()
                    .push_str(&String::from_utf8_lossy(&buffer[..n])),
            }
        }
    });
    wait_until(
        || output.lock().unwrap().contains("PTY-READY"),
        Duration::from_secs(5),
    );
    assert_eq!(
        fs::read_to_string(group_file)
            .unwrap()
            .trim()
            .parse::<u32>()
            .unwrap(),
        child.id()
    );
    master.write_all(b"hello\n").unwrap();
    wait_until(
        || output.lock().unwrap().contains("PTY-WAITING"),
        Duration::from_secs(3),
    );
    master.write_all(&[3]).unwrap();
    wait_until(
        || child.try_wait().unwrap().is_some(),
        Duration::from_secs(5),
    );
    assert!(
        child.wait().unwrap().success(),
        "{}",
        output.lock().unwrap()
    );
    drop(master);
    read_thread.join().unwrap();
    let output = output.lock().unwrap();
    assert!(output.contains("ANSWER:hello"), "{output}");
    assert!(output.contains("PTY-INTERRUPTED"), "{output}");
}

#[test]
fn pty_helper() {
    let Ok(url) = std::env::var("SELFUPDATE_TEST_PTY_URL") else {
        return;
    };
    // Keep the helper alive on terminal Ctrl-C. A caught handler is reset on
    // exec, so Bash and its descendants still receive their normal SIGINT.
    extern "C" fn handle_interrupt(_: i32) {}
    let action = nix::sys::signal::SigAction::new(
        nix::sys::signal::SigHandler::Handler(handle_interrupt),
        nix::sys::signal::SaFlags::empty(),
        nix::sys::signal::SigSet::empty(),
    );
    // SAFETY: the installed signal handler does nothing and is signal-safe.
    unsafe {
        nix::sys::signal::sigaction(Signal::SIGINT, &action).unwrap();
    }
    let error = ScriptInstaller::new(url)
        .install(&CancellationToken::new(), "v1.0.0")
        .unwrap_err();
    assert!(matches!(error, Error::InstallExit(_)), "{error}");
}
