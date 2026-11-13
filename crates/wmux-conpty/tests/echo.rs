use std::io::Read;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use wmux_conpty::{Pty, PtySize};

#[test]
fn echo_round_trips_through_conpty() {
    // Keep cmd.exe alive for ~1 s after the echo. ConPTY's render pump runs
    // out-of-process; on a busy CI runner it can lag cmd.exe's exit by
    // hundreds of ms, and ClosePseudoConsole drops anything still in flight.
    // `ping -n 2 127.0.0.1 >nul` holds the process open without producing
    // console output of its own.
    let (pty, mut reader, _writer) = Pty::spawn(
        r#"cmd.exe /c "echo wmux_echo_token & ping -n 2 127.0.0.1 >nul""#,
        PtySize { cols: 80, rows: 24 },
    )
    .expect("spawn should succeed on Windows 10+");

    // Drain the PTY on a background thread; forward chunks to main.
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });

    // Poll until the token appears or we hit a generous deadline. This is
    // more robust than waiting for cmd.exe to exit and then draining —
    // ConPTY may or may not have flushed by the time the child is gone.
    let mut collected = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(15);
    let found = loop {
        if Instant::now() >= deadline {
            break false;
        }
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(chunk) => {
                collected.extend_from_slice(&chunk);
                if String::from_utf8_lossy(&collected).contains("wmux_echo_token") {
                    break true;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break false,
        }
    };

    drop(pty);

    assert!(
        found,
        "'wmux_echo_token' never appeared in ConPTY output within 15s. Got {} bytes: {:?}",
        collected.len(),
        String::from_utf8_lossy(&collected)
    );
}
