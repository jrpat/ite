//! End-to-end coverage for process/terminal behavior that unit tests cannot
//! model. These tests run the real binary in a PTY while independently piping
//! JSON into stdin or redirecting stdout.
//!
//! The shared harness in `tests/common` acts like a small terminal emulator: it
//! captures screen traffic, answers terminal queries, and sends keys so these
//! tests can verify that output and foreground bindings use the correct stream
//! or controlling terminal.

mod common;

use std::ffi::OsStr;
use std::path::Path;
use std::time::{Duration, Instant};

use common::Session;

const JSON: &str = r#"{"alpha": 1, "beta": [true, null]}"#;

#[test]
fn piped_json_renders_and_enter_prints_the_pointer() {
    let dir = tempfile::tempdir().unwrap();
    let selection = dir.path().join("selection.txt");

    let session = Session::start(
        r#"printf '%s' "$ITE_JSON" | "$ITE_BIN" > "$ITE_SELECTION""#,
        &[
            ("ITE_JSON", OsStr::new(JSON)),
            ("ITE_SELECTION", selection.as_os_str()),
        ],
        dir.path(),
    );
    session.wait_for_render("alpha");
    session.send(b"\r");

    let status = session.wait_for_exit();
    assert!(status.success(), "ite exited with {status:?}");
    assert_eq!(std::fs::read_to_string(&selection).unwrap(), "/alpha\n");
}

#[test]
fn redirected_stdout_alone_still_starts_and_prints_the_selection() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("marker.txt"), "").unwrap();
    let selection = dir.path().join("selection");

    let session = Session::start(
        r#""$ITE_BIN" "$ITE_DIR" > "$ITE_SELECTION""#,
        &[
            ("ITE_DIR", dir.path().as_os_str()),
            ("ITE_SELECTION", selection.as_os_str()),
        ],
        dir.path(),
    );
    session.wait_for_render("marker.txt");
    session.send(b"\r");

    let status = session.wait_for_exit();
    assert!(status.success(), "ite exited with {status:?}");
    let printed = std::fs::read_to_string(&selection).unwrap();
    assert!(
        !printed.contains('\u{1b}'),
        "terminal queries leaked into stdout: {printed:?}"
    );
    assert!(
        printed.trim_end().ends_with("marker.txt"),
        "unexpected selection: {printed:?}"
    );
}

#[test]
fn foreground_binding_reads_the_terminal_not_the_exhausted_pipe() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.toml");
    let ready = dir.path().join("ready");
    let reply = dir.path().join("reply.txt");
    std::fs::write(
        &config,
        r#"
[x]
sh = 'touch "$ITE_READY"; IFS= read -r line; printf "%s" "$line" > "$ITE_REPLY"'
exit = true
"#,
    )
    .unwrap();

    let session = Session::start(
        r#"printf '%s' "$ITE_JSON" | "$ITE_BIN" -c "$ITE_CONFIG""#,
        &[
            ("ITE_JSON", OsStr::new(JSON)),
            ("ITE_CONFIG", config.as_os_str()),
            ("ITE_READY", ready.as_os_str()),
            ("ITE_REPLY", reply.as_os_str()),
        ],
        dir.path(),
    );
    session.wait_for_render("alpha");
    session.send(b"x");
    wait_for_file(&ready);
    session.send(b"typed on the terminal\r");

    let status = session.wait_for_exit();
    assert!(status.success(), "ite exited with {status:?}");
    assert_eq!(
        std::fs::read_to_string(&reply).unwrap(),
        "typed on the terminal"
    );
}

fn wait_for_file(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !path.exists() {
        assert!(
            Instant::now() < deadline,
            "{} never appeared",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}
