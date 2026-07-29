//! Shared PTY harness for end-to-end tests: runs the real binary inside a
//! PTY through `sh -c` and answers the terminal queries ite depends on
//! (cursor position, device attributes) like a real emulator would.

use std::ffi::OsStr;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

/// An ite process inside a PTY, launched through `sh -c` so the script can
/// pipe JSON into its stdin and redirect its stdout to a file.
pub struct Session {
    child: Box<dyn Child + Send + Sync>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    transcript: Arc<Mutex<Vec<u8>>>,
    _master: Box<dyn MasterPty + Send>,
}

impl Session {
    pub fn start(script: &str, envs: &[(&str, &OsStr)], cwd: &Path) -> Self {
        let pty = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("openpty");
        let mut cmd = CommandBuilder::new("sh");
        cmd.args(["-c", script]);
        cmd.env("ITE_BIN", env!("CARGO_BIN_EXE_ite"));
        for (name, value) in envs {
            cmd.env(name, value);
        }
        cmd.cwd(cwd);
        let child = pty.slave.spawn_command(cmd).expect("spawn sh");
        drop(pty.slave);

        let mut reader = pty.master.try_clone_reader().expect("pty reader");
        let writer = Arc::new(Mutex::new(pty.master.take_writer().expect("pty writer")));
        let transcript = Arc::new(Mutex::new(Vec::new()));
        {
            let writer = Arc::clone(&writer);
            let transcript = Arc::clone(&transcript);
            std::thread::spawn(move || {
                let mut buf = [0u8; 8192];
                let mut tail: Vec<u8> = Vec::new();
                while let Ok(n) = reader.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    transcript.lock().unwrap().extend_from_slice(&buf[..n]);
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

        Self {
            child,
            writer,
            transcript,
            _master: pty.master,
        }
    }

    pub fn send(&self, bytes: &[u8]) {
        let mut writer = self.writer.lock().unwrap();
        writer.write_all(bytes).expect("write to pty");
        writer.flush().expect("flush pty");
    }

    pub fn wait_for_render(&self, needle: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let snapshot = self.transcript.lock().unwrap().clone();
            if find(&snapshot, needle.as_bytes()).is_some() {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "{needle:?} never rendered; terminal output:\n{}",
                String::from_utf8_lossy(&snapshot)
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    pub fn wait_for_exit(mut self) -> portable_pty::ExitStatus {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = self.child.try_wait().expect("try_wait") {
                return status;
            }
            if Instant::now() > deadline {
                let _ = self.child.kill();
                panic!("ite did not exit");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}
