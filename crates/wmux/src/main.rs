#![forbid(unsafe_code)]

use std::io::{Read, Write};

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use tokio::net::windows::named_pipe::ClientOptions;
use tokio::sync::mpsc;
use tracing_subscriber::EnvFilter;

use wmux_ipc::{read_frame, write_frame, PIPE_NAME};
use wmux_proto::{ClientMsg, Dir, FocusDir, Hello, PtySize, ServerMsg, Verb, PROTOCOL_VERSION};

#[derive(Parser)]
#[command(
    name = "wmux",
    version,
    about = "wmux client — a Windows terminal multiplexer"
)]
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

    let snapshot_bytes = match read_frame::<ServerMsg, _>(&mut read_half).await? {
        ServerMsg::GridSnapshot(bytes) => bytes,
        other => bail!("expected GridSnapshot, got {other:?}"),
    };

    crossterm::terminal::enable_raw_mode().context("entering raw mode")?;
    let _guard = RawModeGuard;

    {
        let mut out = std::io::stdout().lock();
        out.write_all(&snapshot_bytes)?;
        let _ = out.write_all(
            b"\r\n[wmux] attached. Ctrl+A d=detach \" vsplit % hsplit hjkl=focus x=kill\r\n",
        );
        out.flush()?;
    }

    let (stdin_tx, mut stdin_rx) = mpsc::channel::<StdinEvent>(32);
    std::thread::spawn(move || stdin_pump(stdin_tx));

    loop {
        tokio::select! {
            msg = read_frame::<ServerMsg, _>(&mut read_half) => {
                match msg {
                    Ok(ServerMsg::Output(bytes)) => {
                        let mut out = std::io::stdout().lock();
                        out.write_all(&bytes)?;
                        out.flush()?;
                    }
                    Ok(ServerMsg::GridSnapshot(bytes)) => {
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
                    Some(StdinEvent::Command(verb)) => {
                        if write_frame(&mut write_half, &ClientMsg::Command(verb)).await.is_err() {
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

// Blocking stdin reader + prefix handler. Ctrl+A (0x01) arms the prefix;
// the next byte is interpreted as a wmux command. Anything that isn't a
// recognised command after Ctrl+A is forwarded along with the Ctrl+A, so
// the shell still sees Ctrl+A when typed twice or paired with an unbound
// key.
fn stdin_pump(tx: mpsc::Sender<StdinEvent>) {
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
                        let cmd: Option<StdinEvent> = match b {
                            b'd' => Some(StdinEvent::Detach),
                            b'"' => Some(StdinEvent::Command(Verb::SplitPane(Dir::V))),
                            b'%' => Some(StdinEvent::Command(Verb::SplitPane(Dir::H))),
                            b'h' => Some(StdinEvent::Command(Verb::FocusPane(FocusDir::Left))),
                            b'j' => Some(StdinEvent::Command(Verb::FocusPane(FocusDir::Down))),
                            b'k' => Some(StdinEvent::Command(Verb::FocusPane(FocusDir::Up))),
                            b'l' => Some(StdinEvent::Command(Verb::FocusPane(FocusDir::Right))),
                            b'x' => Some(StdinEvent::Command(Verb::KillPane)),
                            PREFIX => {
                                out.push(PREFIX);
                                None
                            }
                            other => {
                                out.push(PREFIX);
                                out.push(other);
                                None
                            }
                        };
                        if let Some(evt) = cmd {
                            if !out.is_empty() {
                                let _ =
                                    tx.blocking_send(StdinEvent::Bytes(std::mem::take(&mut out)));
                            }
                            let is_detach = matches!(evt, StdinEvent::Detach);
                            if tx.blocking_send(evt).is_err() {
                                return;
                            }
                            if is_detach {
                                return;
                            }
                        }
                    } else if b == PREFIX {
                        armed = true;
                    } else {
                        out.push(b);
                    }
                }
                if !out.is_empty() && tx.blocking_send(StdinEvent::Bytes(out)).is_err() {
                    return;
                }
            }
        }
    }
}

enum StdinEvent {
    Bytes(Vec<u8>),
    Command(Verb),
    Detach,
}

struct RawModeGuard;
impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
    }
}
