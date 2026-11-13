// Smoke test for M1: spawn `cmd.exe /c echo hello`, pump it through ConPTY,
// print the raw bytes that come back. Run with:
//
//     cargo run -p wmux-conpty --example cmd_echo

use std::io::Read;
use std::time::Duration;

use wmux_conpty::{Pty, PtySize};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (pty, mut reader, _writer) = Pty::spawn(
        "cmd.exe /c echo hello from ConPTY",
        PtySize { cols: 80, rows: 24 },
    )?;

    // Reader thread: drain the PTY output pipe until the pseudoconsole closes.
    let reader_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = reader.read_to_end(&mut buf);
        buf
    });

    let exit = pty.wait(Duration::from_secs(5))?;
    println!("child exit code: {:?}", exit);
    println!("child pid: {}", pty.pid());

    // ConPTY's render pump needs a beat to flush the child's final output
    // onto our pipe — ClosePseudoConsole drops anything still in flight.
    std::thread::sleep(Duration::from_millis(500));

    // Dropping the Pty calls ClosePseudoConsole, which closes the output pipe
    // and unblocks the reader thread above.
    drop(pty);

    let bytes = reader_thread.join().expect("reader thread panicked");
    println!("--- {} bytes of raw PTY output ---", bytes.len());
    print!("{}", String::from_utf8_lossy(&bytes));
    println!();
    println!("--- end ---");

    Ok(())
}
