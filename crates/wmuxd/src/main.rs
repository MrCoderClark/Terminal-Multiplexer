#![forbid(unsafe_code)]

mod compositor;
mod session;

use std::sync::Arc;

use anyhow::{bail, Context, Result};
use clap::Parser;
use tokio::io::AsyncWriteExt;
use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use tokio::sync::Mutex as AsyncMutex;
use tracing::{debug, error, info, warn};
use tracing_subscriber::EnvFilter;

use wmux_ipc::{read_frame, write_frame, PIPE_NAME};
use wmux_proto::{ClientMsg, ServerMsg, Verb, Welcome, PROTOCOL_VERSION};

use crate::session::Session;

struct DaemonState {
    session: AsyncMutex<Option<Session>>,
}

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

    let state = Arc::new(DaemonState {
        session: AsyncMutex::new(None),
    });

    let mut server = ServerOptions::new()
        .first_pipe_instance(true)
        .create(PIPE_NAME)
        .with_context(|| format!("creating named pipe {PIPE_NAME}"))?;

    loop {
        server
            .connect()
            .await
            .context("waiting for client connect")?;
        info!("client connected");

        let this_client = server;
        server = ServerOptions::new()
            .create(PIPE_NAME)
            .with_context(|| format!("re-creating named pipe {PIPE_NAME}"))?;

        let state = state.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_client(this_client, state).await {
                error!(error = %e, "client session ended with error");
            } else {
                info!("client session ended cleanly");
            }
        });
    }
}

async fn handle_client(pipe: NamedPipeServer, state: Arc<DaemonState>) -> Result<()> {
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

    // --- find or create session ----------------------------------------
    let session: Session = {
        let mut guard = state.session.lock().await;
        let reuse = guard.as_ref().map(|s| !s.is_dead()).unwrap_or(false);
        if !reuse {
            info!("spawning fresh session");
            *guard = Some(Session::spawn(hello.term_size)?);
        } else if let Some(ref s) = *guard {
            if let Err(e) = s.resize(hello.term_size) {
                warn!(error = %e, "resize on attach failed");
            }
        }
        guard.as_ref().unwrap().clone()
    };

    let (mut output_rx, snapshot_bytes) = session.attach();

    write_frame(
        &mut write_half,
        &ServerMsg::Welcome(Welcome {
            session_id: session.id(),
            protocol_version: PROTOCOL_VERSION,
        }),
    )
    .await
    .context("sending Welcome")?;

    write_frame(&mut write_half, &ServerMsg::GridSnapshot(snapshot_bytes))
        .await
        .context("sending GridSnapshot")?;

    // --- main loop: pipe ↔ session -------------------------------------
    loop {
        tokio::select! {
            msg = read_frame::<ClientMsg, _>(&mut read_half) => {
                match msg {
                    Ok(ClientMsg::Input(bytes)) => {
                        if session.send_input_to_focused(bytes).is_err() {
                            break;
                        }
                    }
                    Ok(ClientMsg::Resize(size)) => {
                        if let Err(e) = session.resize(size) {
                            warn!(error = %e, "resize failed");
                        }
                    }
                    Ok(ClientMsg::Command(verb)) => {
                        if let Err(e) = handle_verb(&session, verb) {
                            warn!(error = %e, "command failed");
                        }
                    }
                    Ok(ClientMsg::Bye) => {
                        debug!("client sent Bye");
                        break;
                    }
                    Ok(ClientMsg::Hello(_)) => warn!("unexpected second Hello — ignoring"),
                    Err(e) => {
                        debug!(error = %e, "client read ended");
                        break;
                    }
                }
            }
            chunk = output_rx.recv() => {
                match chunk {
                    Some(bytes) => {
                        if write_frame(&mut write_half, &ServerMsg::Output(bytes))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    None => {
                        let _ = write_frame(
                            &mut write_half,
                            &ServerMsg::Bye("session closed".into()),
                        )
                        .await;
                        break;
                    }
                }
            }
        }
    }

    session.detach();
    let _ = write_half.shutdown().await;
    Ok(())
}

fn handle_verb(session: &Session, verb: Verb) -> Result<()> {
    match verb {
        Verb::SplitPane(dir) => session.split_focused(dir),
        Verb::FocusPane(dir) => {
            session.focus_move(dir);
            Ok(())
        }
        Verb::KillPane => {
            session.kill_focused();
            Ok(())
        }
    }
}
