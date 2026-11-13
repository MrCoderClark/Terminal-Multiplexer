#![forbid(unsafe_code)]

use std::io::{Read, Write};

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use tokio::net::windows::named_pipe::ClientOptions;
use tokio::sync::mpsc;
use tracing_subscriber::EnvFilter;

use wmux_ipc::{read_frame, write_frame, PIPE_NAME};
use wmux_proto::{ClientMsg, Hello, PtySize, ServerMsg, PROTOCOL_VERSION};

#[derive(Parser)]
#[command(name = "wmux", version, about = "wmux client — a Windows terminal multiplexer")]
struct Cli {
    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Attach to the running wmuxd and open a shell pane.
    Attach,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .init();

    let cli = Cli::parse();
    match cli.command.unwrap_or(Cmd::Attach) {
        Cmd::Attach => {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            rt.block_on(attach())
        }
    }
}

async fn attach() -> Result<()> {
    let pipe = ClientOptions::new()
        .open(PIPE_NAME)
        .with_context(|| format!("connecting to {PIPE_NAME} — is wmuxd running?"))?;

    let (mut read_half, mut write_half) = tokio::io::split(pipe);

    // --- handshake ------------------------------------------------------
    let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
    write_frame(
        &mut write_half,
        &ClientMsg::Hello(Hello {
            client_version: env!("CARGO_PKG_VERSION").to_string(),
            protocol_version: PROTOCOL_VERSION,
            term_size: PtySize { cols, rows },
        }),
    )
    .await
    .context("sending Hello")?;

    match read_frame::<ServerMsg, _>(&mut read_half).await? {
        ServerMsg::Welcome(_) => {}
        ServerMsg::ProtocolMismatch { server, client } => {
            bail!(
                "protocol version mismatch (server {server}, client {client}); upgrade one of them"
            );
        }
        other => bail!("expected Welcome, got {other:?}"),
    }

    // --- raw mode -------------------------------------------------------
    crossterm::terminal::enable_raw_mode().context("entering raw mode")?;
    let _guard = RawModeGuard;

    // Banner printed inside raw mode needs explicit CR + LF.
    {
        let mut out = std::io::stdout().lock();
        let _ = out.write_all(b"[wmux] attached. Detach: Ctrl+A d\r\n");
        let _ = out.flush();
    }

    // --- stdin → event channel -----------------------------------------
    // Blocking stdin read on an OS thread. The thread also implements the
    // minimal prefix handler: 0x01 (Ctrl+A) arms, 'd' after arming means
    // detach, anything else after arming is forwarded along with the Ctrl+A
    // so the shell still sees Ctrl+A when typed twice or followed by a key
    // that's not a wmux command.
    let (stdin_tx, mut stdin_rx) = mpsc::channel::<StdinEvent>(32);
    std::thread::spawn(move || {
        const PREFIX: u8 = 0x01; // Ctrl+A
        let stdin = std::io::stdin();
        let mut lock = stdin.lock();
        let mut buf = [0u8; 1024];
        let mut armed = false;

        loop {
            match lock.read(&mut buf) {
                Ok(0) | Err(_) => return,
                Ok(n) => {
                    let mut out = Vec::with_capacity(n);
                    for &b in &buf[..n] {
                        if armed {
                            armed = false;
                            match b {
                                b'd' => {
                                    if !out.is_empty() {
                                        let _ =
                                            stdin_tx.blocking_send(StdinEvent::Bytes(out.clone()));
                                    }
                                    let _ = stdin_tx.blocking_send(StdinEvent::Detach);
                                    return;
                                }
                                PREFIX => out.push(PREFIX),
                                _ => {
                                    out.push(PREFIX);
                                    out.push(b);
                                }
                            }
                        } else if b == PREFIX {
                            armed = true;
                        } else {
                            out.push(b);
                        }
                    }
                    if !out.is_empty()
                        && stdin_tx.blocking_send(StdinEvent::Bytes(out)).is_err()
                    {
                        return;
                    }
                }
            }
        }
    });

    // --- main loop: multiplex pipe reads and stdin events --------------
    loop {
        tokio::select! {
            msg = read_frame::<ServerMsg, _>(&mut read_half) => {
                match msg {
                    Ok(ServerMsg::Output(bytes)) => {
                        let mut out = std::io::stdout().lock();
                        out.write_all(&bytes)?;
                        out.flush()?;
                    }
                    Ok(ServerMsg::Bye(_)) => break,
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
            evt = stdin_rx.recv() => {
                match evt {
                    Some(StdinEvent::Bytes(bytes)) => {
                        if write_frame(&mut write_half, &ClientMsg::Input(bytes)).await.is_err() {
                            break;
                        }
                    }
                    Some(StdinEvent::Detach) => {
                        let _ = write_frame(&mut write_half, &ClientMsg::Bye).await;
                        break;
                    }
                    None => break,
                }
            }
        }
    }

    Ok(())
}

enum StdinEvent {
    Bytes(Vec<u8>),
    Detach,
}

// Restore the terminal when attach() returns, panics, or is cancelled.
struct RawModeGuard;
impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
    }
}
