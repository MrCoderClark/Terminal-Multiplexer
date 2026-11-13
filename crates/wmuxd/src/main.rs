#![forbid(unsafe_code)]

use std::io::{Read, Write};

use anyhow::{bail, Context, Result};
use clap::Parser;
use tokio::io::AsyncWriteExt;
use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};
use tracing_subscriber::EnvFilter;

use wmux_conpty::{Pty, PtySize as ConptySize};
use wmux_ipc::{read_frame, write_frame, PIPE_NAME};
use wmux_proto::{ClientMsg, Hello, PtySize, ServerMsg, Welcome, PROTOCOL_VERSION};

#[derive(Parser)]
#[command(name = "wmuxd", version, about = "wmux daemon")]
struct Cli {
    /// Run in the foreground instead of detaching. Logs stream to stderr.
    #[arg(long)]
    foreground: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // The daemon never writes to its own stdout — see CLAUDE.md hard rule 4.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    info!(foreground = cli.foreground, "wmuxd starting");

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("building tokio runtime")?;

    runtime.block_on(run())
}

async fn run() -> Result<()> {
    info!(pipe = PIPE_NAME, "listening on named pipe");

    let mut server = ServerOptions::new()
        .first_pipe_instance(true)
        .create(PIPE_NAME)
        .with_context(|| format!("creating named pipe {PIPE_NAME}"))?;

    // M2 serves one client at a time: after the current attach finishes,
    // we loop back and open another pipe instance. Multi-attach lands with
    // the session registry in M3/M4.
    loop {
        server.connect().await.context("waiting for client connect")?;
        info!("client connected");

        // Hand the connected instance to the handler and immediately prepare
        // the next instance so a fresh client won't see "pipe busy".
        let this_client = server;
        server = ServerOptions::new()
            .create(PIPE_NAME)
            .with_context(|| format!("re-creating named pipe {PIPE_NAME}"))?;

        if let Err(e) = handle_client(this_client).await {
            error!(error = %e, "client session ended with error");
        } else {
            info!("client session ended cleanly");
        }
    }
}

async fn handle_client(pipe: NamedPipeServer) -> Result<()> {
    let (mut read_half, mut write_half) = tokio::io::split(pipe);

    // --- handshake ------------------------------------------------------
    let first: ClientMsg = read_frame(&mut read_half).await.context("reading Hello")?;
    let hello = match first {
        ClientMsg::Hello(h) => h,
        other => bail!("expected Hello, got {:?}", other),
    };

    debug!(?hello, "received Hello");

    if hello.protocol_version != PROTOCOL_VERSION {
        warn!(
            server = PROTOCOL_VERSION,
            client = hello.protocol_version,
            "protocol version mismatch — rejecting"
        );
        let _ = write_frame(
            &mut write_half,
            &ServerMsg::ProtocolMismatch {
                server: PROTOCOL_VERSION,
                client: hello.protocol_version,
            },
        )
        .await;
        return Ok(());
    }

    // --- spawn the pane -------------------------------------------------
    let Hello { term_size, .. } = hello;
    let (pty, pty_reader, pty_writer) = Pty::spawn(
        "cmd.exe",
        ConptySize {
            cols: term_size.cols,
            rows: term_size.rows,
        },
    )
    .context("spawning ConPTY")?;

    info!(pid = pty.pid(), "spawned cmd.exe");

    write_frame(
        &mut write_half,
        &ServerMsg::Welcome(Welcome {
            session_id: pty.pid() as u64,
            protocol_version: PROTOCOL_VERSION,
        }),
    )
    .await
    .context("sending Welcome")?;

    // --- pumps ----------------------------------------------------------
    // PTY reads and writes are blocking file I/O. Spin two OS threads that
    // bridge to tokio via channels so we can mix them with the async pipe.
    let (output_tx, mut output_rx) = mpsc::channel::<Vec<u8>>(64);
    let reader_thread = std::thread::spawn(move || pump_pty_to_channel(pty_reader, output_tx));

    let (input_tx, input_rx) = std::sync::mpsc::channel::<Vec<u8>>();
    let writer_thread = std::thread::spawn(move || pump_channel_to_pty(pty_writer, input_rx));

    // Task: forward PTY output chunks → ServerMsg::Output frames.
    let output_task = tokio::spawn(async move {
        while let Some(bytes) = output_rx.recv().await {
            if write_frame(&mut write_half, &ServerMsg::Output(bytes))
                .await
                .is_err()
            {
                break;
            }
        }
        // Signal client-side the session is closing. Best-effort.
        let _ = write_frame(&mut write_half, &ServerMsg::Bye("pty closed".into())).await;
        let _ = write_half.shutdown().await;
    });

    // Main loop: read ClientMsg frames; dispatch.
    let mut pty_holder = Some(pty);
    loop {
        match read_frame::<ClientMsg, _>(&mut read_half).await {
            Ok(ClientMsg::Input(bytes)) => {
                if input_tx.send(bytes).is_err() {
                    debug!("pty writer thread gone");
                    break;
                }
            }
            Ok(ClientMsg::Resize(size)) => {
                if let Some(ref pty) = pty_holder {
                    if let Err(e) = pty.resize(ConptySize {
                        cols: size.cols,
                        rows: size.rows,
                    }) {
                        warn!(error = %e, "resize failed");
                    }
                }
            }
            Ok(ClientMsg::Bye) => {
                debug!("client sent Bye");
                break;
            }
            Ok(ClientMsg::Hello(_)) => {
                warn!("unexpected second Hello — ignoring");
            }
            Err(e) => {
                debug!(error = %e, "client read ended");
                break;
            }
        }
    }

    // Tearing down: drop Pty (closes ConPTY → reader thread EOFs), drop
    // input channel (writer thread exits), await the output task.
    drop(input_tx);
    let _ = writer_thread.join();
    drop(pty_holder.take());
    let _ = reader_thread.join();
    let _ = output_task.await;

    Ok(())
}

fn pump_pty_to_channel(mut reader: wmux_conpty::PtyReader, tx: mpsc::Sender<Vec<u8>>) {
    let mut buf = [0u8; 4096];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if tx.blocking_send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
}

fn pump_channel_to_pty(mut writer: wmux_conpty::PtyWriter, rx: std::sync::mpsc::Receiver<Vec<u8>>) {
    while let Ok(bytes) = rx.recv() {
        if writer.write_all(&bytes).is_err() {
            break;
        }
        let _ = writer.flush();
    }
}
