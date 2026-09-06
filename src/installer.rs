use crate::{CancellationToken, Result};
use std::fs::File;

/// Replaces the binary with the specified release. Implementations must check
/// cancellation before side effects and may cooperate during installation.
pub trait Installer {
    /// Install the original, unnormalized release tag.
    fn install(&mut self, token: &CancellationToken, tag: &str) -> Result<()>;
}
impl<T: Installer + ?Sized> Installer for &mut T {
    fn install(&mut self, token: &CancellationToken, tag: &str) -> Result<()> {
        (**self).install(token, tag)
    }
}
impl<T: Installer + ?Sized> Installer for Box<T> {
    fn install(&mut self, token: &CancellationToken, tag: &str) -> Result<()> {
        (**self).install(token, tag)
    }
}

/// Reusable process stream configuration, defaulting to inherited stdio.
/// Use files for capturing output; this API does not create undrained pipes.
#[derive(Debug, Default)]
pub enum ProcessStdio {
    /// Attach the caller's corresponding stream.
    #[default]
    Inherit,
    /// Use the null device.
    Null,
    /// Duplicate this file descriptor for each installation.
    File(File),
}
impl From<File> for ProcessStdio {
    fn from(file: File) -> Self {
        Self::File(file)
    }
}

/// Downloads a trusted install script with curl and executes it using Bash.
/// Supported on macOS and Linux; other platforms return `UnsupportedPlatform`.
#[derive(Debug)]
#[cfg_attr(not(any(target_os = "linux", target_os = "macos")), allow(dead_code))]
pub struct ScriptInstaller {
    url: String,
    stdin: ProcessStdio,
    stdout: ProcessStdio,
    stderr: ProcessStdio,
}
impl ScriptInstaller {
    /// Configure a script accepting `--force --tag=<tag>`.
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            stdin: ProcessStdio::Inherit,
            stdout: ProcessStdio::Inherit,
            stderr: ProcessStdio::Inherit,
        }
    }
    /// Configure stdin; real terminals retain the caller's process group.
    pub fn stdin(mut self, stream: impl Into<ProcessStdio>) -> Self {
        self.stdin = stream.into();
        self
    }
    /// Configure stdout independently of the updater's status output.
    pub fn stdout(mut self, stream: impl Into<ProcessStdio>) -> Self {
        self.stdout = stream.into();
        self
    }
    /// Configure stderr independently of the updater's warning output.
    pub fn stderr(mut self, stream: impl Into<ProcessStdio>) -> Self {
        self.stderr = stream.into();
        self
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod supported {
    use super::*;
    use crate::Error;
    use nix::{
        errno::Errno,
        sys::signal::{Signal, killpg},
        unistd::{Pid, getpgrp},
    };
    use std::{
        io::{self, IsTerminal},
        os::unix::process::CommandExt,
        process::{Child, Command, Stdio},
        thread,
        time::{Duration, Instant},
    };

    // URL and tag are data, never shell source. `--` prevents curl option injection.
    const PROGRAM: &str = "curl -fsSL -- \"$1\" | bash -s -- --force \"--tag=$2\"";
    const POLL: Duration = Duration::from_millis(25);
    const GRACE: Duration = Duration::from_secs(5);

    impl ProcessStdio {
        fn stdio(&self) -> io::Result<Stdio> {
            match self {
                Self::Inherit => Ok(Stdio::inherit()),
                Self::Null => Ok(Stdio::null()),
                Self::File(f) => f.try_clone().map(Stdio::from),
            }
        }
        fn stdin_is_terminal(&self) -> bool {
            match self {
                Self::Inherit => io::stdin().is_terminal(),
                Self::Null => false,
                Self::File(f) => f.is_terminal(),
            }
        }
    }

    fn signal_group(group: Pid, signal: Option<Signal>) -> io::Result<bool> {
        // Never signal zero, a negative PID, or the caller's process group.
        if group.as_raw() <= 0 || group == getpgrp() {
            return Err(io::Error::other(
                "refusing to signal the caller's process group",
            ));
        }
        match killpg(group, signal) {
            Ok(()) => Ok(true),
            Err(Errno::ESRCH) => Ok(false),
            Err(e) => Err(io::Error::from_raw_os_error(e as i32)),
        }
    }

    fn terminate(child: &mut Child, group: Option<Pid>) -> io::Result<()> {
        let Some(group) = group else {
            // Terminal jobs share our group. Only stop/reap the direct child;
            // terminal-generated Ctrl-C reaches the whole foreground group.
            if child.try_wait()?.is_none() {
                child.kill()?;
            }
            return Ok(());
        };
        let deadline = Instant::now() + GRACE;
        let mut signal = Some(Signal::SIGTERM);
        loop {
            // Reap the leader promptly, but keep checking the group: descendants
            // can outlive Bash, including children that ignore SIGTERM.
            child.try_wait()?;
            match signal_group(group, signal) {
                Ok(false) => break,
                Ok(true) => signal = None,
                // Darwin's killpg skips zombies and returns EPERM if no live
                // members remain. Wait for reaping within the existing grace
                // period; persistent permission errors must still propagate.
                Err(error)
                    if cfg!(target_os = "macos")
                        && error.raw_os_error() == Some(Errno::EPERM as i32)
                        && Instant::now() < deadline => {}
                Err(error) => return Err(error),
            }
            if Instant::now() >= deadline {
                signal_group(group, Some(Signal::SIGKILL))?;
                break;
            }
            thread::sleep(POLL);
        }
        Ok(())
    }

    fn stop(child: &mut Child, group: Option<Pid>) -> io::Result<()> {
        let result = terminate(child, group);
        if result.is_err() {
            // Even when monitoring or signalling fails, make a best effort to
            // stop the owned processes and always attempt to reap our child.
            if let Some(group) = group {
                let _ = signal_group(group, Some(Signal::SIGKILL));
            }
            let _ = child.kill();
        }
        let waited = child.wait().map(|_| ());
        result.and(waited)
    }

    impl Installer for ScriptInstaller {
        fn install(&mut self, token: &CancellationToken, tag: &str) -> Result<()> {
            token.check()?;
            if self.url.is_empty() {
                return Err(Error::Configuration("ScriptInstaller URL is empty".into()));
            }
            if tag.is_empty()
                || !tag.as_bytes()[0].is_ascii_alphanumeric()
                || !tag
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'+' | b'-'))
            {
                return Err(Error::UnsafeTag(tag.into()));
            }
            let detached = !self.stdin.stdin_is_terminal();
            let mut command = Command::new("bash");
            command
                .args([
                    "-o",
                    "pipefail",
                    "-c",
                    PROGRAM,
                    "selfupdate-rs",
                    &self.url,
                    tag,
                ])
                .stdin(self.stdin.stdio().map_err(Error::Process)?)
                .stdout(self.stdout.stdio().map_err(Error::Process)?)
                .stderr(self.stderr.stdio().map_err(Error::Process)?);
            if detached {
                command.process_group(0);
            }
            token.check()?;
            let mut child = command.spawn().map_err(Error::Process)?;
            let group = detached.then(|| Pid::from_raw(child.id() as i32));
            loop {
                if token.is_cancelled() {
                    stop(&mut child, group).map_err(Error::Process)?;
                    return Err(Error::Cancelled);
                }
                match child.try_wait() {
                    Ok(Some(status)) => {
                        if token.is_cancelled() {
                            stop(&mut child, group).map_err(Error::Process)?;
                            return Err(Error::Cancelled);
                        }
                        return if status.success() {
                            Ok(())
                        } else {
                            Err(Error::InstallExit(status))
                        };
                    }
                    Ok(None) => thread::sleep(POLL),
                    Err(error) => {
                        stop(&mut child, group).map_err(Error::Process)?;
                        return Err(Error::Process(error));
                    }
                }
            }
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
impl Installer for ScriptInstaller {
    fn install(&mut self, token: &CancellationToken, _tag: &str) -> Result<()> {
        token.check()?;
        Err(crate::Error::UnsupportedPlatform)
    }
}
