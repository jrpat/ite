//! The span profiler must be compiled out of shipped builds: without the
//! `profile` cargo feature the binary contains no instrumentation and
//! ignores `ITE_PROFILE` entirely. With the feature (what `cargo
//! profile-tui` builds), the same run writes the span table. Both tests
//! drive the real binary in a PTY because the dump happens at exit.
#![cfg(unix)]

mod common;

use common::Session;

fn run_with_ite_profile(profile: &std::path::Path) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("marker.txt"), "").unwrap();

    let session = Session::start(
        r#""$ITE_BIN" "$ITE_DIR""#,
        &[
            ("ITE_DIR", dir.path().as_os_str()),
            ("ITE_PROFILE", profile.as_os_str()),
        ],
        dir.path(),
    );
    session.wait_for_render("marker.txt");
    session.send(b"q");
    session.wait_for_exit();
}

#[cfg(not(feature = "profile"))]
#[test]
fn default_build_ignores_ite_profile() {
    let out = tempfile::tempdir().unwrap();
    let profile = out.path().join("profile.txt");
    run_with_ite_profile(&profile);
    assert!(
        !profile.exists(),
        "a build without the profile feature wrote a span dump"
    );
}

#[cfg(feature = "profile")]
#[test]
fn profile_build_writes_the_span_table() {
    let out = tempfile::tempdir().unwrap();
    let profile = out.path().join("profile.txt");
    run_with_ite_profile(&profile);
    let dump = std::fs::read_to_string(&profile).expect("span dump written at exit");
    assert!(dump.contains("main::frame"), "unexpected dump:\n{dump}");
}
