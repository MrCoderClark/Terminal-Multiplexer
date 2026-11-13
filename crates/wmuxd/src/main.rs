#![forbid(unsafe_code)]

use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{bail, Context, Result};
use clap::Parser;
use tokio::io::AsyncWriteExt;
use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use tokio::sync::{mpsc, Mutex as AsyncMutex};
use tracing::{debug, error, info, warn};
use tracing_subscriber::EnvFilter;

use wmux_conpty::{Pty, PtySize as ConptySize};
use wmux_core::Terminal;
use wmux_ipc::{read_frame, write_frame, PIPE_NAME};
use wmux_proto::{ClientMsg, PtySize, ServerMsg, Welcome, PROTOCOL_VERSION};

// ---------------------------------------------------------------------------
// Session
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct Session {
    inner: Arc<SessionInner>,
}

struct SessionInner {
    pty: Mutex<Pty>,
    terminal: Mutex<Terminal>,
    input_tx: std::sync::mpsc::Sender<Vec<u8>>,
    // Set to Some(sender) while a client is attached. The reader thread
    // forwards every chunk it reads from the PTY both into the terminal
    // (always) and into this channel (if any client is listening).
    active_client: Mutex<Option<mpsc::UnboundedSender<Vec<u8>>>>,
    dead: AtomicBool,
    pid: u32,
}

impl Session {
    fn spawn(size: PtySize) -> Result<Self> {
        let (pty, mut pty_reader, mut pty_writer) = Pty::spawn(
            "cmd.exe",
            ConptySize {
                cols: size.cols,
                rows: size.rows,
            },
        )
        .context("spawning ConPTY")?;
        let pid = pty.pid();

        let terminal = Terminal::new(size.cols, size.rows);
        let (input_tx, input_rx) = std::sync::mpsc::channel::<Vec<u8>>();

        let inner = Arc::new(SessionInner {
            pty: Mutex::new(pty),
            terminal: Mutex::new(terminal),
            input_tx,
            active_client: Mutex::new(None),
            dead: AtomicBool::new(false),
            pid,
        });

        // Reader thread: PTY → Terminal + active client.
        {
            let inner = inner.clone();
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                loop {
                    match pty_reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            let chunk = buf[..n].to_vec();
                            inner.terminal.lock().unwrap().feed(&chunk);
                            let client = inner.active_client.lock().unwrap().clone();
                            if let Some(tx) = client {
                                let _ = tx.send(chunk);
                            }
                        }
                    }
                }
                debug!(pid = inner.pid, "pty reader thread exiting");
                inner.dead.store(true, Ordering::Release);
                *inner.active_client.lock().unwrap() = None;
            });
        }

        // Writer thread: input_rx → PTY.
        std::thread::spawn(move || {
            while let Ok(bytes) = input_rx.recv() {
                if pty_writer.write_all(&bytes).is_err() {
                    break;
                }
                let _ = pty_writer.flush();
            }
            debug!("pty writer thread exiting");
        });

        info!(pid, cols = size.cols, rows = size.rows, "spawned session");

        Ok(Self { inner })
    }

    fn is_dead(&self) -> bool {
        self.inner.dead.load(Ordering::Acquire)
    }

    fn pid(&self) -> u32 {
        self.inner.pid
    }

    // Registers a fresh client channel and returns the receive half plus a
    // snapshot of the current screen. If a stale client was still registered,
    // it is dropped — only one attachment at a time in M3.
    fn attach(&self) -> (mpsc::UnboundedReceiver<Vec<u8>>, Vec<u8>) {
        let (tx, rx) = mpsc::unbounded_channel();
        *self.inner.active_client.lock().unwrap() = Some(tx);
        let snapshot = self.inner.terminal.lock().unwrap().snapshot();
        (rx, snapshot)
    }

    fn detach(&self) {
        *self.inner.active_client.lock().unwrap() = None;
    }

    fn send_input(&self, bytes: Vec<u8>) -> Result<()> {
        self.inner
            .input_tx
            .send(bytes)
            .map_err(|_| anyhow::anyhow!("session writer thread is gone"))
    }

    fn resize(&self, size: PtySize) -> Result<()> {
        self.inner
            .pty
            .lock()
            .unwrap()
            .resize(ConptySize {
                cols: size.cols,
                rows: size.rows,
            })
            .context("ResizePseudoConsole")?;
        self.inner
            .terminal
            .lock()
            .unwrap()
            .resize(size.cols, size.rows);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Daemon
// ---------------------------------------------------------------------------

struct DaemonState {
    // One session for M3. The registry lands with M4/M7.
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

        // Prepare the next pipe instance before handing this one off so a
        // fresh client doesn't see "pipe busy".
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
            // The reattaching client may have a different terminal size
            // than the previous one; resync the ConPTY and the virtual grid.
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
            session_id: session.pid() as u64,
            protocol_version: PROTOCOL_VERSION,
        }),
    )
    .await
    .context("sending Welcome")?;

    write_frame(&mut write_half, &ServerMsg::GridSnapshot(snapshot_bytes))
        .await
        .context("sending GridSnapshot")?;

    // --- main loop: pipe ↔ session -------------------------------------
    let mut graceful_bye = false;
    loop {
        tokio::select! {
            msg = read_frame::<ClientMsg, _>(&mut read_half) => {
                match msg {
                    Ok(ClientMsg::Input(bytes)) => {
                        if session.send_input(bytes).is_err() {
                            break;
                        }
                    }
                    Ok(ClientMsg::Resize(size)) => {
                        if let Err(e) = session.resize(size) {
                            warn!(error = %e, "resize failed");
                        }
                    }
                    Ok(ClientMsg::Bye) => {
                        debug!("client sent Bye");
                        graceful_bye = true;
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
                        // PTY closed. Session is dead — tell the client.
                        let _ = write_frame(
                            &mut write_half,
                            &ServerMsg::Bye("pty closed".into()),
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
    debug!(graceful_bye, "handle_client returning");
    Ok(())
}
