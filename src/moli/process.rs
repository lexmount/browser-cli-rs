//! Ownership checks for CLI-created Moli processes. No process-name or port scan is used.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::Child;

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub start_identity: String,
    pub executable: PathBuf,
}

/// Read a live process owned by the current user, including its kernel birth identity.
pub fn identity(pid: u32) -> Result<ProcessIdentity> {
    platform::identity(pid)
}

/// Fail closed if the process exited, changed executable, or cannot be inspected.
pub fn matches(expected: &ProcessIdentity) -> bool {
    identity(expected.pid).is_ok_and(|actual| actual == *expected)
}

/// Spawn a separate session, with stdio independent of the invoking CLI.
/// The caller owns the private log and performs bounded HTTP/CDP readiness checks.
pub fn spawn(binary: &Path, stderr: &File) -> Result<(Child, ProcessIdentity)> {
    platform::spawn(binary, stderr)
}

/// Stop only the recorded process. This never signals a process group or escalates to KILL.
pub fn stop(expected: &ProcessIdentity) -> Result<()> {
    platform::stop(expected)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
mod platform {
    use super::*;
    use std::os::unix::fs::MetadataExt;
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::thread;
    use std::time::{Duration, Instant};

    const STARTUP_TIMEOUT: Duration = Duration::from_secs(1);
    const STOP_TIMEOUT: Duration = Duration::from_secs(3);
    const POLL_INTERVAL: Duration = Duration::from_millis(25);

    // Moli needs no cloud credentials or desktop IPC authority. Proxy configuration is
    // deliberately inherited, rather than bypassing the caller's network policy.
    const PRIVATE_ENV: &[&str] = &[
        "LEXMOUNT_API_KEY",
        "LEXMOUNT_PROJECT_ID",
        "LEXMOUNT_BASE_URL",
        "LEXMOUNT_REGION",
        "LEXMOUNT_BROWSER_CREDENTIALS_FILE",
        "LEXMOUNT_AUTH_SOURCE",
        "LEXMOUNT_DESKTOP_EXPECTED_PROJECT_ID",
        "LEXMOUNT_DESKTOP_EXPECTED_API_BASE_URL",
        "LEXMOUNT_APP_EVENTS",
        "LEXMOUNT_APP_EVENT_ENDPOINT",
        "LEXMOUNT_APP_EVENT_TOKEN",
        "LEXMOUNT_APP_CONNECTION_FILE",
        "LEXMOUNT_AGENT_RUN_ID",
        "OPENAI_API_KEY",
        "CODEX_API_KEY",
        "ANTHROPIC_API_KEY",
        "AZURE_OPENAI_API_KEY",
        "GOOGLE_API_KEY",
        "GEMINI_API_KEY",
    ];

    pub(super) fn identity(pid: u32) -> Result<ProcessIdentity> {
        let pid_arg = checked_pid(pid)?;
        let before = birth_identity(pid_arg)?;
        let executable = executable(pid_arg)?;
        // Recheck after reading the executable to reject exit/reuse during inspection.
        let after = birth_identity(pid_arg)?;
        if before != after {
            return Err(Error::Conflict(
                "Moli process changed during inspection".into(),
            ));
        }
        Ok(ProcessIdentity {
            pid,
            start_identity: before,
            executable,
        })
    }

    pub(super) fn spawn(binary: &Path, stderr: &File) -> Result<(Child, ProcessIdentity)> {
        if !binary.is_absolute() {
            return Err(Error::Config(
                "Moli executable must be an absolute path".into(),
            ));
        }
        let binary = binary.canonicalize()?;
        let metadata = binary.metadata()?;
        if !metadata.is_file() || metadata.mode() & 0o111 == 0 || metadata.mode() & 0o6000 != 0 {
            return Err(Error::Config(
                "Moli executable must be a regular, non-setuid executable".into(),
            ));
        }
        let log = stderr.metadata()?;
        if !log.is_file() || log.uid() != unsafe { libc::geteuid() } || log.mode() & 0o077 != 0 {
            return Err(Error::Config(
                "Moli startup log must be a private file owned by the current user".into(),
            ));
        }
        let mut command = Command::new(&binary);
        command
            .args([
                "serve",
                "--host",
                "127.0.0.1",
                "--port",
                "0",
                "--layout",
                "--image",
                "--font",
                "--screencast-interval-ms",
                "100",
                "--log-level",
                "info",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(stderr.try_clone()?));
        for name in PRIVATE_ENV {
            command.env_remove(name);
        }
        // Only async-signal-safe libc is called after fork and before exec. setsid
        // detaches from the CLI's terminal/session without a shell or daemon wrapper.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    Err(std::io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
        let mut child = command.spawn()?;
        let deadline = Instant::now() + STARTUP_TIMEOUT;
        loop {
            match child.try_wait() {
                Ok(Some(_)) => {
                    return Err(Error::Config(
                        "Moli exited before its process identity was verified".into(),
                    ));
                }
                Ok(None) => {}
                Err(error) => {
                    discard_child(&mut child);
                    return Err(Error::Io(error));
                }
            }
            match identity(child.id()) {
                Ok(actual) if actual.executable == binary => return Ok((child, actual)),
                Ok(_) => {
                    discard_child(&mut child);
                    return Err(Error::Config(
                        "Moli executable resolved to a different process image".into(),
                    ));
                }
                Err(_) if Instant::now() < deadline => thread::sleep(POLL_INTERVAL),
                Err(_) => {
                    discard_child(&mut child);
                    return Err(Error::Config(
                        "Moli process identity could not be verified".into(),
                    ));
                }
            }
        }
    }

    fn discard_child(child: &mut Child) {
        // An unreaped child PID cannot be reused. This cleanup is only for a spawn
        // that has not been handed to the registry, never for persisted identities.
        let _ = child.kill();
        let _ = child.wait();
    }

    pub(super) fn stop(expected: &ProcessIdentity) -> Result<()> {
        if expected.pid <= 1 || expected.pid == std::process::id() {
            return Err(Error::Config(
                "Refusing to stop an invalid Moli process identity".into(),
            ));
        }
        signal_owned(expected)?;
        let deadline = Instant::now() + STOP_TIMEOUT;
        loop {
            match identity(expected.pid) {
                Ok(current) if current == *expected => {}
                Ok(_) | Err(Error::NotFound(_)) => return Ok(()),
                Err(error) => return Err(error),
            }
            if Instant::now() >= deadline {
                return Err(Error::Timeout(
                    "Moli did not exit after termination was requested".into(),
                ));
            }
            thread::sleep(POLL_INTERVAL);
        }
    }

    fn require_match(expected: &ProcessIdentity) -> Result<()> {
        if identity(expected.pid)? != *expected {
            return Err(Error::Conflict(
                "Moli process identity no longer matches; no signal was sent".into(),
            ));
        }
        Ok(())
    }

    fn checked_pid(pid: u32) -> Result<libc::pid_t> {
        if pid == 0 || pid > i32::MAX as u32 {
            return Err(Error::Config("Invalid Moli process ID".into()));
        }
        Ok(pid as libc::pid_t)
    }

    fn missing() -> Error {
        Error::NotFound("Moli process is no longer running".into())
    }

    fn inspection_error(error: std::io::Error) -> Error {
        match error.raw_os_error() {
            Some(libc::ESRCH) | Some(libc::ENOENT) => missing(),
            _ => Error::Io(error),
        }
    }

    #[cfg(target_os = "macos")]
    fn birth_identity(pid: libc::pid_t) -> Result<String> {
        let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::uninit();
        let size = std::mem::size_of::<libc::proc_bsdinfo>();
        let read = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTBSDINFO,
                0,
                info.as_mut_ptr().cast(),
                size as i32,
            )
        };
        if read != size as i32 {
            return Err(inspection_error(std::io::Error::last_os_error()));
        }
        let info = unsafe { info.assume_init() };
        let uid = unsafe { libc::geteuid() };
        if info.pbi_pid != pid as u32 || info.pbi_status == libc::SZOMB {
            return Err(missing());
        }
        if info.pbi_uid != uid || info.pbi_ruid != uid || info.pbi_svuid != uid {
            return Err(Error::Config(
                "Moli process is not owned by the current user".into(),
            ));
        }
        Ok(format!(
            "macos:{}:{}",
            info.pbi_start_tvsec, info.pbi_start_tvusec
        ))
    }

    #[cfg(target_os = "macos")]
    fn executable(pid: libc::pid_t) -> Result<PathBuf> {
        use std::os::unix::ffi::OsStrExt;
        let mut buffer = vec![0_u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
        let length =
            unsafe { libc::proc_pidpath(pid, buffer.as_mut_ptr().cast(), buffer.len() as u32) };
        if length <= 0 {
            return Err(inspection_error(std::io::Error::last_os_error()));
        }
        let length = length as usize;
        if length >= buffer.len() {
            return Err(Error::Config(
                "Moli executable path exceeds the inspection limit".into(),
            ));
        }
        let end = buffer[..length]
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(length);
        PathBuf::from(std::ffi::OsStr::from_bytes(&buffer[..end]))
            .canonicalize()
            .map_err(inspection_error)
    }

    #[cfg(target_os = "macos")]
    fn signal_owned(expected: &ProcessIdentity) -> Result<()> {
        // macOS has no pidfd equivalent for kill. Check the kernel birth identity
        // immediately before the PID-based signal; never retry against a new PID.
        require_match(expected)?;
        if unsafe { libc::kill(checked_pid(expected.pid)?, libc::SIGTERM) } == -1 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                return Err(Error::Io(error));
            }
        }
        Ok(())
    }

    #[cfg(target_os = "linux")]
    fn birth_identity(pid: libc::pid_t) -> Result<String> {
        use std::io::Read;
        let directory = PathBuf::from(format!("/proc/{pid}"));
        let uid = unsafe { libc::geteuid() };
        if directory.metadata().map_err(inspection_error)?.uid() != uid {
            return Err(Error::Config(
                "Moli process is not owned by the current user".into(),
            ));
        }
        let mut stat = String::new();
        File::open(directory.join("stat"))
            .map_err(inspection_error)?
            .take(8193)
            .read_to_string(&mut stat)?;
        if stat.len() > 8192 {
            return Err(Error::Config(
                "Moli process information exceeds the inspection limit".into(),
            ));
        }
        let ticks = linux_start_ticks(&stat, pid as u32)?;
        let mut boot_id = String::new();
        File::open("/proc/sys/kernel/random/boot_id")?
            .take(129)
            .read_to_string(&mut boot_id)?;
        let boot_id = boot_id.trim();
        if boot_id.len() != 36
            || !boot_id
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
        {
            return Err(Error::Config(
                "Linux boot identity could not be verified".into(),
            ));
        }
        Ok(format!("linux:{boot_id}:{ticks}"))
    }

    #[cfg(target_os = "linux")]
    fn executable(pid: libc::pid_t) -> Result<PathBuf> {
        std::fs::read_link(format!("/proc/{pid}/exe"))
            .and_then(|path| path.canonicalize())
            .map_err(inspection_error)
    }

    #[cfg(target_os = "linux")]
    fn signal_owned(expected: &ProcessIdentity) -> Result<()> {
        use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
        let pid = checked_pid(expected.pid)?;
        // Pin the kernel process before validation. If the PID is recycled, this
        // descriptor still refers only to the old process, never its replacement.
        let descriptor = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
        if descriptor == -1 {
            return Err(inspection_error(std::io::Error::last_os_error()));
        }
        let descriptor = unsafe { OwnedFd::from_raw_fd(descriptor as i32) };
        require_match(expected)?;
        let result = unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                descriptor.as_raw_fd(),
                libc::SIGTERM,
                std::ptr::null::<libc::siginfo_t>(),
                0,
            )
        };
        if result == -1 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                return Err(Error::Io(error));
            }
        }
        Ok(())
    }

    #[cfg(target_os = "linux")]
    fn linux_start_ticks(stat: &str, expected_pid: u32) -> Result<u64> {
        let invalid = || Error::Config("Invalid Linux process identity".into());
        let open = stat.find('(').ok_or_else(invalid)?;
        let close = stat
            .rfind(')')
            .filter(|close| *close > open)
            .ok_or_else(invalid)?;
        if stat[..open].trim().parse::<u32>().ok() != Some(expected_pid) {
            return Err(invalid());
        }
        let fields: Vec<_> = stat[close + 1..].split_whitespace().collect();
        // Field 3 is state; field 22 is starttime. comm may contain spaces or ')'.
        if fields
            .first()
            .is_some_and(|state| *state == "Z" || *state == "X")
        {
            return Err(missing());
        }
        fields
            .get(19)
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| *value > 0)
            .ok_or_else(invalid)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn current_process_has_a_stable_kernel_identity() {
            let first = identity(std::process::id()).unwrap();
            assert!(first.executable.is_absolute());
            assert!(!first.start_identity.is_empty());
            assert_eq!(identity(std::process::id()).unwrap(), first);
            assert!(super::super::matches(&first));
        }

        #[test]
        fn changed_birth_or_executable_never_matches() {
            let mut current = identity(std::process::id()).unwrap();
            current.start_identity.push_str(":stale");
            assert!(!super::super::matches(&current));
            let mut current = identity(std::process::id()).unwrap();
            current.executable = PathBuf::from("/not-a-moli-process");
            assert!(!super::super::matches(&current));
            assert!(identity(0).is_err());
            assert!(identity(u32::MAX).is_err());
        }

        #[cfg(target_os = "linux")]
        #[test]
        fn linux_stat_parser_handles_parentheses_in_process_name() {
            let tail: Vec<String> = (3..=22)
                .map(|field| match field {
                    3 => "S".into(),
                    22 => "123456".into(),
                    _ => "0".into(),
                })
                .collect();
            let stat = format!("42 (name with ) parentheses) {}", tail.join(" "));
            assert_eq!(linux_start_ticks(&stat, 42).unwrap(), 123456);
            assert!(linux_start_ticks(&stat, 43).is_err());
            assert!(linux_start_ticks("42 (bad) S", 42).is_err());
            let zombie = stat.replacen(") S ", ") Z ", 1);
            assert!(matches!(
                linux_start_ticks(&zombie, 42),
                Err(Error::NotFound(_))
            ));
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod platform {
    use super::*;

    fn unsupported<T>() -> Result<T> {
        Err(Error::Config(
            "CLI-owned Moli processes are supported on macOS and Linux only".into(),
        ))
    }
    pub(super) fn identity(_pid: u32) -> Result<ProcessIdentity> {
        unsupported()
    }
    pub(super) fn spawn(_binary: &Path, _stderr: &File) -> Result<(Child, ProcessIdentity)> {
        unsupported()
    }
    pub(super) fn stop(_expected: &ProcessIdentity) -> Result<()> {
        unsupported()
    }
}
