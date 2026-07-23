//! Executes `sh` keybindings with `$path` / `$relpath` in the environment.

use std::ffi::OsStr;
use std::process::{Command, ExitStatus, Stdio};

/// Run `cmd` through `sh -c`. The focused node's action values are exported as
/// `$path` and `$relpath`.
///
/// Foreground commands inherit the terminal and their exit status is returned;
/// background commands are detached from stdio and `None` is returned.
pub fn run_shell(
    cmd: &str,
    path: &OsStr,
    relpath: &OsStr,
    bg: bool,
) -> std::io::Result<Option<ExitStatus>> {
    let mut command = Command::new("sh");
    command
        .arg("-c")
        .arg(cmd)
        .env("path", path)
        .env("relpath", relpath);
    if bg {
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        Ok(None)
    } else {
        command.status().map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn foreground_command_sees_path_env_vars() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out");
        let status = run_shell(
            &format!("printf '%s\\n%s' \"$path\" \"$relpath\" > {}", out.display()),
            OsStr::new("/abs/some/file.txt"),
            OsStr::new("some/file.txt"),
            false,
        )
        .unwrap()
        .expect("foreground returns a status");
        assert!(status.success());
        assert_eq!(
            std::fs::read_to_string(out).unwrap(),
            "/abs/some/file.txt\nsome/file.txt"
        );
    }

    #[test]
    fn foreground_reports_failure_status() {
        let status = run_shell("exit 3", OsStr::new("/x"), OsStr::new("x"), false)
            .unwrap()
            .unwrap();
        assert_eq!(status.code(), Some(3));
    }

    #[test]
    fn background_command_detaches() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out");
        let result = run_shell(
            &format!("echo \"$relpath\" > {}", out.display()),
            OsStr::new("/abs/f"),
            OsStr::new("f"),
            true,
        )
        .unwrap();
        assert!(result.is_none());
        // The detached process still runs to completion.
        for _ in 0..50 {
            if out.exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(std::fs::read_to_string(out).unwrap().trim(), "f");
    }
}
