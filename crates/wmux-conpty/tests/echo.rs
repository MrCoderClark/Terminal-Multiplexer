use std::io::Read;
use std::thread;
use std::time::Duration;

use wmux_conpty::{Pty, PtySize};

#[test]
fn echo_round_trips_through_conpty() {
    let (pty, mut reader, _writer) = Pty::spawn(
        "cmd.exe /c echo wmux_echo_token",
        PtySize { cols: 80, rows: 24 },
    )
    .expect("spawn should succeed on Windows 10+");

    let reader_thread = thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = reader.read_to_end(&mut buf);
        buf
    });

    let exit = pty
        .wait(Duration::from_secs(10))
        .expect("wait should not error");
    assert!(exit.is_some(), "cmd.exe did not exit in 10s");

    // ConPTY batches rendering: the child's stdout bytes have to pass
    // through ConPTY's internal pump before they land on our output pipe.
    // ClosePseudoConsole tears that pump down abruptly and drops anything
    // still in flight, so give it a beat after the child exits.
    thread::sleep(Duration::from_millis(500));

    drop(pty);

    let bytes = reader_thread.join().expect("reader thread panicked");
    let text = String::from_utf8_lossy(&bytes);
    assert!(
        text.contains("wmux_echo_token"),
        "expected 'wmux_echo_token' in ConPTY output.\nexit code: {:?}\n{} bytes: {:?}",
        exit,
        bytes.len(),
        text
    );
}
