//! Subprocess boundary for the platform's default "open this path" program.
//! `app` decides *what* to open; this module knows *how*, as one table arm per
//! operating system.
//!
//! The opener is spawned detached from ite's standard streams: some handlers
//! return at once and others linger for the lifetime of the application they
//! launch, and neither should write over the TUI or hold up the event loop.

use std::ffi::OsStr;
use std::process::{Command, Stdio};

/// The program that hands a path to a platform's default handler, plus the
/// fixed arguments that precede the path.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Opener {
    pub program: &'static str,
    pub args: &'static [&'static str],
}

/// The opener for an [`std::env::consts::OS`] value; `None` where ite does not
/// know of one. Supporting another platform is one more arm.
pub fn opener_for(os: &str) -> Option<Opener> {
    let opener = match os {
        "macos" => Opener {
            program: "open",
            args: &[],
        },
        "linux" | "freebsd" | "netbsd" | "openbsd" | "dragonfly" | "solaris" | "illumos" => {
            Opener {
                program: "xdg-open",
                args: &[],
            }
        }
        // `start` is a shell builtin rather than a program, and it reads a
        // leading quoted argument as the new window's title; the empty string
        // keeps the path a path. Untested: no Windows target ships today (see
        // dist-workspace.toml), and whoever adds one should check how `cmd`
        // re-splits the path it is handed.
        "windows" => Opener {
            program: "cmd",
            args: &["/C", "start", ""],
        },
        _ => return None,
    };
    Some(opener)
}

/// The opener for the platform this binary runs on.
pub fn opener() -> Option<Opener> {
    opener_for(std::env::consts::OS)
}

/// Hand `path` to the platform's default handler. The error is a message fit
/// for the user, not a cause to stop exploring.
pub fn open(path: &OsStr) -> Result<(), String> {
    open_with(opener(), path)
}

fn open_with(opener: Option<Opener>, path: &OsStr) -> Result<(), String> {
    let opener =
        opener.ok_or_else(|| format!("no default opener known for {}", std::env::consts::OS))?;
    spawn(opener, path).map_err(|error| {
        format!(
            "cannot open {} with {}: {error}",
            path.to_string_lossy(),
            opener.program
        )
    })
}

/// Spawn `opener` on `path`, detached from ite's standard streams and left to
/// outlive the event loop.
fn spawn(opener: Opener, path: &OsStr) -> std::io::Result<()> {
    Command::new(opener.program)
        .args(opener.args)
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_known_platform_names_its_opener() {
        assert_eq!(
            opener_for("macos"),
            Some(Opener {
                program: "open",
                args: &[]
            })
        );
        assert_eq!(
            opener_for("linux"),
            Some(Opener {
                program: "xdg-open",
                args: &[]
            })
        );
        assert_eq!(
            opener_for("windows"),
            Some(Opener {
                program: "cmd",
                args: &["/C", "start", ""]
            })
        );
    }

    #[test]
    fn the_bsds_share_the_freedesktop_opener() {
        for os in ["freebsd", "netbsd", "openbsd", "dragonfly", "illumos"] {
            assert_eq!(opener_for(os).unwrap().program, "xdg-open", "{os}");
        }
    }

    #[test]
    fn an_unknown_platform_has_no_opener() {
        assert_eq!(opener_for("haiku"), None);
    }

    #[test]
    fn the_platform_ite_is_built_for_has_an_opener() {
        assert!(opener().is_some(), "{}", std::env::consts::OS);
    }

    #[test]
    fn the_path_is_the_final_argument_and_the_child_is_detached() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("marker");
        // A stand-in opener: writes the path it was handed next to itself.
        let opener = Opener {
            program: "sh",
            args: &["-c", "printf %s \"$0\" > \"$(dirname \"$0\")/out\""],
        };

        spawn(opener, target.as_os_str()).unwrap();

        let out = dir.path().join("out");
        for _ in 0..50 {
            if out.exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(
            std::fs::read_to_string(out).unwrap(),
            target.display().to_string()
        );
    }

    #[test]
    fn an_unstartable_opener_names_the_path_and_the_program() {
        let error = open_with(
            Some(Opener {
                program: "ite-nonexistent-opener",
                args: &[],
            }),
            OsStr::new("/some/file"),
        )
        .unwrap_err();
        assert!(error.contains("/some/file"), "{error}");
        assert!(error.contains("ite-nonexistent-opener"), "{error}");
    }

    #[test]
    fn a_platform_without_an_opener_says_so_rather_than_guessing() {
        let error = open_with(None, OsStr::new("/some/file")).unwrap_err();
        assert!(error.contains("no default opener"), "{error}");
    }
}
