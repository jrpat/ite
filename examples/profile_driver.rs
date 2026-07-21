//! Headless TUI profiler: runs the release `ite` binary in a real PTY,
//! simulates keypresses, and reports round-trip latency per key plus the
//! app's internal span profile (via ITE_PROFILE).
//!
//! Usage: cargo profile-tui [PATH] [ITERS]
//!   PATH   directory to explore (default ".")
//!   ITERS  repetitions for the j/k phases (default 15)

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ite::profile::{Stats, format_duration};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};

/// Bytes seen from the app, updated by the reader thread.
#[derive(Default)]
struct Output {
    bytes: u64,
    last_at: Option<Instant>,
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or_else(|| ".".to_string());
    let iters: usize = args.next().map(|s| s.parse().expect("ITERS must be a number")).unwrap_or(15);

    let build = std::process::Command::new("cargo")
        .args(["build", "--release", "--bin", "ite", "-q"])
        .status()
        .expect("cargo build");
    assert!(build.success(), "release build failed");

    let profile_path = std::env::temp_dir().join(format!("ite-profile-{}.txt", std::process::id()));

    let pty = native_pty_system()
        .openpty(PtySize {
            rows: 40,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("openpty");
    let ite_bin = std::env::current_dir().unwrap().join("target/release/ite");
    let mut cmd = CommandBuilder::new(ite_bin);
    cmd.args(["-e", "all", &path]);
    cmd.env("ITE_PROFILE", &profile_path);
    cmd.cwd(std::env::current_dir().unwrap());
    let mut child = pty.slave.spawn_command(cmd).expect("spawn ite");
    drop(pty.slave);

    let mut reader = pty.master.try_clone_reader().expect("pty reader");
    let writer = Arc::new(Mutex::new(pty.master.take_writer().expect("pty writer")));
    let output = Arc::new(Mutex::new(Output::default()));

    // Read everything ite draws; answer the terminal queries it depends on
    // (cursor position, device attributes) like a real emulator would.
    {
        let output = Arc::clone(&output);
        let writer = Arc::clone(&writer);
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            let mut tail: Vec<u8> = Vec::new();
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 {
                    break;
                }
                {
                    let mut out = output.lock().unwrap();
                    out.bytes += n as u64;
                    out.last_at = Some(Instant::now());
                }
                tail.extend_from_slice(&buf[..n]);
                let mut respond = Vec::new();
                while let Some(i) = find(&tail, b"\x1b[6n") {
                    respond.extend_from_slice(b"\x1b[1;1R");
                    tail.drain(..i + 4);
                }
                while let Some(i) = find(&tail, b"\x1b[c") {
                    respond.extend_from_slice(b"\x1b[?1;2c");
                    tail.drain(..i + 3);
                }
                if !respond.is_empty() {
                    let mut w = writer.lock().unwrap();
                    let _ = w.write_all(&respond);
                    let _ = w.flush();
                }
                if tail.len() > 64 {
                    let excess = tail.len() - 64;
                    tail.drain(..excess);
                }
            }
        });
    }

    // Wait for the first frame.
    let startup = Instant::now();
    while output.lock().unwrap().bytes == 0 && startup.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(1));
    }
    wait_quiet(&output, Duration::from_millis(150), Duration::from_secs(10));
    println!("startup to first stable frame: {}", format_duration(startup.elapsed()));
    println!();

    let jk: Vec<&[u8]> = [b"j", b"k"].into_iter().cycle().take(iters * 2).map(|b| b.as_slice()).collect();
    let lh: Vec<&[u8]> = [b"l", b"h"].into_iter().cycle().take(20).map(|b| b.as_slice()).collect();
    let phases: Vec<(&str, Vec<&[u8]>)> = vec![
        ("j (down)", vec![b"j"; iters]),
        ("k (up)", vec![b"k"; iters]),
        ("j/k alternate", jk),
        ("l/h toggle", lh),
        ("H (collapse-rec)", vec![b"H"]),
        ("L (expand-rec)", vec![b"L"]),
        ("G then gg", vec![b"G", b"gg"]),
    ];

    println!(
        "{:<18} {:>5} {:>6} {:>9} {:>9} {:>9} {:>9} {:>10}",
        "phase", "keys", "silent", "mean", "p50", "p95", "max", "bytes/key"
    );
    for (name, keys) in phases {
        let mut latencies = Vec::new();
        let mut silent = 0usize;
        let mut bytes_total = 0u64;
        for key in &keys {
            match send_key(&writer, &output, key) {
                Some((latency, bytes)) => {
                    latencies.push(latency);
                    bytes_total += bytes;
                }
                None => silent += 1,
            }
        }
        match Stats::from_durations(&latencies) {
            Some(s) => println!(
                "{:<18} {:>5} {:>6} {:>9} {:>9} {:>9} {:>9} {:>10}",
                name,
                keys.len(),
                silent,
                format_duration(s.mean),
                format_duration(s.p50),
                format_duration(s.p95),
                format_duration(s.max),
                bytes_total / latencies.len().max(1) as u64,
            ),
            None => println!("{name:<18} {:>5} {silent:>6} (no visual response)", keys.len()),
        }
    }

    // Quit and collect the app's internal profile.
    {
        let mut w = writer.lock().unwrap();
        let _ = w.write_all(b"q");
        let _ = w.flush();
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if child.try_wait().expect("try_wait").is_some() {
            break;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            eprintln!("warning: ite did not exit after q; killed");
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    println!("\n== internal spans (ITE_PROFILE) ==");
    match std::fs::read_to_string(&profile_path) {
        Ok(dump) => print!("{dump}"),
        Err(e) => println!("(missing: {e})"),
    }
    let _ = std::fs::remove_file(&profile_path);
}

/// Send one key (or chord) and wait for the redraw it causes.
/// Returns first-byte latency and bytes emitted, or `None` if the app stayed
/// silent (a no-op key produces an empty diff and writes nothing).
fn send_key(
    writer: &Arc<Mutex<Box<dyn Write + Send>>>,
    output: &Arc<Mutex<Output>>,
    key: &[u8],
) -> Option<(Duration, u64)> {
    let before = output.lock().unwrap().bytes;
    let sent_at = Instant::now();
    {
        let mut w = writer.lock().unwrap();
        w.write_all(key).expect("write key");
        w.flush().expect("flush key");
    }
    let latency = loop {
        if output.lock().unwrap().bytes > before {
            break sent_at.elapsed();
        }
        if sent_at.elapsed() > Duration::from_millis(300) {
            return None;
        }
        std::thread::sleep(Duration::from_micros(200));
    };
    wait_quiet(output, Duration::from_millis(5), Duration::from_secs(2));
    let total = output.lock().unwrap().bytes - before;
    Some((latency, total))
}

/// Wait until the app's last output is at least `quiet` old, or `timeout`
/// elapses.
fn wait_quiet(output: &Arc<Mutex<Output>>, quiet: Duration, timeout: Duration) {
    let start = Instant::now();
    loop {
        {
            let out = output.lock().unwrap();
            if out.last_at.is_some_and(|t| t.elapsed() >= quiet) {
                return;
            }
        }
        if start.elapsed() > timeout {
            return;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}
