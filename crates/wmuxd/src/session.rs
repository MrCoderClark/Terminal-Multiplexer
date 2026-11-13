use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Context, Result};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use wmux_conpty::{Pty, PtyReader, PtySize as ConptySize};
use wmux_core::{Dir, FocusDir, PaneId, PaneTree, Rect, Terminal};
use wmux_proto::PtySize;

use crate::compositor;

// Per-pane resources: the ConPTY, its virtual terminal, and the writer-thread
// channel. Dropping a `PaneState` closes ConPTY (via `Pty`'s Drop), which
// signals the reader thread to EOF and exit.
pub struct PaneState {
    pub pty: Mutex<Pty>,
    pub terminal: Mutex<Terminal>,
    pub input_tx: std::sync::mpsc::Sender<Vec<u8>>,
}

// Entire session state. Locked as one unit so split/focus/kill don't race
// against the compositor reading the tree.
pub struct SessionState {
    pub tree: PaneTree,
    pub panes: HashMap<PaneId, Arc<PaneState>>,
    pub focused: PaneId,
    pub size: PtySize,
}

pub struct SessionInner {
    pub state: Mutex<SessionState>,
    pub active_client: Mutex<Option<mpsc::UnboundedSender<Vec<u8>>>>,
    pub dirty: AtomicBool,
    pub dead: AtomicBool,
    pub id: u64,
    next_pane_id: AtomicU32,
}

#[derive(Clone)]
pub struct Session {
    inner: Arc<SessionInner>,
}

impl Session {
    pub fn spawn(size: PtySize) -> Result<Self> {
        let initial_id: PaneId = 1;
        let (pane, reader) = spawn_pane(size)?;
        let session_id = pane.pty.lock().unwrap().pid() as u64;

        let mut panes = HashMap::new();
        panes.insert(initial_id, pane);

        let inner = Arc::new(SessionInner {
            state: Mutex::new(SessionState {
                tree: PaneTree::leaf(initial_id),
                panes,
                focused: initial_id,
                size,
            }),
            active_client: Mutex::new(None),
            dirty: AtomicBool::new(true),
            dead: AtomicBool::new(false),
            id: session_id,
            next_pane_id: AtomicU32::new(initial_id + 1),
        });

        let pane_arc = inner.state.lock().unwrap().panes[&initial_id].clone();
        spawn_reader_thread(initial_id, pane_arc, inner.clone(), reader);

        tokio::spawn(compositor::run(inner.clone()));

        info!(
            session_id,
            cols = size.cols,
            rows = size.rows,
            "spawned session"
        );

        Ok(Self { inner })
    }

    pub fn id(&self) -> u64 {
        self.inner.id
    }

    pub fn is_dead(&self) -> bool {
        self.inner.dead.load(Ordering::Acquire)
    }

    pub fn attach(&self) -> (mpsc::UnboundedReceiver<Vec<u8>>, Vec<u8>) {
        let (tx, rx) = mpsc::unbounded_channel();
        *self.inner.active_client.lock().unwrap() = Some(tx);
        self.inner.dirty.store(true, Ordering::Release);
        let snapshot = compositor::compose(&self.inner);
        (rx, snapshot)
    }

    pub fn detach(&self) {
        *self.inner.active_client.lock().unwrap() = None;
    }

    pub fn send_input_to_focused(&self, bytes: Vec<u8>) -> Result<()> {
        let state = self.inner.state.lock().unwrap();
        let focused = state.focused;
        let pane = state
            .panes
            .get(&focused)
            .ok_or_else(|| anyhow!("focused pane {focused} vanished"))?
            .clone();
        drop(state);
        pane.input_tx
            .send(bytes)
            .map_err(|_| anyhow!("pane {focused} writer thread is gone"))
    }

    pub fn resize(&self, size: PtySize) -> Result<()> {
        let mut state = self.inner.state.lock().unwrap();
        state.size = size;
        let rects = state.tree.compute_rects(Rect {
            x: 0,
            y: 0,
            w: size.cols,
            h: size.rows,
        });
        for (pane_id, rect) in rects {
            if let Some(pane) = state.panes.get(&pane_id) {
                let _ = pane.pty.lock().unwrap().resize(ConptySize {
                    cols: rect.w.max(1),
                    rows: rect.h.max(1),
                });
                pane.terminal
                    .lock()
                    .unwrap()
                    .resize(rect.w.max(1), rect.h.max(1));
            }
        }
        self.inner.dirty.store(true, Ordering::Release);
        Ok(())
    }

    pub fn split_focused(&self, dir: Dir) -> Result<()> {
        let new_id = self.inner.next_pane_id.fetch_add(1, Ordering::SeqCst);
        let mut state = self.inner.state.lock().unwrap();
        let target = state.focused;

        let total = Rect {
            x: 0,
            y: 0,
            w: state.size.cols,
            h: state.size.rows,
        };
        let mut speculative_tree = state.tree.clone();
        if !speculative_tree.split_leaf(target, new_id, dir) {
            return Err(anyhow!("focused pane {target} not found in tree"));
        }
        let rects = speculative_tree.compute_rects(total);
        let new_rect = rects
            .iter()
            .find(|(id, _)| *id == new_id)
            .map(|(_, r)| *r)
            .ok_or_else(|| anyhow!("split did not produce new pane"))?;
        let sib_rect = rects.iter().find(|(id, _)| *id == target).map(|(_, r)| *r);

        let (new_pane, reader) = spawn_pane(PtySize {
            cols: new_rect.w.max(1),
            rows: new_rect.h.max(1),
        })?;
        state.panes.insert(new_id, new_pane.clone());
        state.tree = speculative_tree;
        state.focused = new_id;

        // Shrink the original (still-focused-until-now) sibling.
        if let Some(sib_rect) = sib_rect {
            if let Some(sib_pane) = state.panes.get(&target) {
                let _ = sib_pane.pty.lock().unwrap().resize(ConptySize {
                    cols: sib_rect.w.max(1),
                    rows: sib_rect.h.max(1),
                });
                sib_pane
                    .terminal
                    .lock()
                    .unwrap()
                    .resize(sib_rect.w.max(1), sib_rect.h.max(1));
            }
        }

        drop(state);

        spawn_reader_thread(new_id, new_pane, self.inner.clone(), reader);
        self.inner.dirty.store(true, Ordering::Release);
        info!(new_id, ?dir, "split pane");
        Ok(())
    }

    pub fn focus_move(&self, dir: FocusDir) {
        let mut state = self.inner.state.lock().unwrap();
        let area = Rect {
            x: 0,
            y: 0,
            w: state.size.cols,
            h: state.size.rows,
        };
        if let Some(next) = state.tree.focus_from(state.focused, dir, area) {
            state.focused = next;
            self.inner.dirty.store(true, Ordering::Release);
        }
    }

    pub fn kill_focused(&self) {
        let mut state = self.inner.state.lock().unwrap();
        let target = state.focused;
        if state.panes.len() <= 1 {
            state.panes.clear();
            self.inner.dead.store(true, Ordering::Release);
            // Dropping the active_client sender here closes the attached
            // client's output_rx, which lets handle_client send Bye and exit.
            *self.inner.active_client.lock().unwrap() = None;
            self.inner.dirty.store(true, Ordering::Release);
            info!("killed last pane — session ending");
            return;
        }
        if !state.tree.remove(target) {
            warn!(target, "kill: pane not removable");
            return;
        }
        state.panes.remove(&target);

        let new_focused = *state.panes.keys().next().unwrap();
        state.focused = new_focused;

        let rects = state.tree.compute_rects(Rect {
            x: 0,
            y: 0,
            w: state.size.cols,
            h: state.size.rows,
        });
        for (pane_id, rect) in rects {
            if let Some(pane) = state.panes.get(&pane_id) {
                let _ = pane.pty.lock().unwrap().resize(ConptySize {
                    cols: rect.w.max(1),
                    rows: rect.h.max(1),
                });
                pane.terminal
                    .lock()
                    .unwrap()
                    .resize(rect.w.max(1), rect.h.max(1));
            }
        }

        self.inner.dirty.store(true, Ordering::Release);
        info!(target, "killed pane");
    }
}

// Create the ConPTY, its virtual terminal, and the writer thread. The reader
// is returned so the caller can wire it up to the SessionInner it belongs to.
fn spawn_pane(size: PtySize) -> Result<(Arc<PaneState>, PtyReader)> {
    let (pty, pty_reader, mut pty_writer) = Pty::spawn(
        "cmd.exe",
        ConptySize {
            cols: size.cols.max(1),
            rows: size.rows.max(1),
        },
    )
    .context("spawning ConPTY")?;
    let terminal = Terminal::new(size.cols.max(1), size.rows.max(1));
    let (input_tx, input_rx) = std::sync::mpsc::channel::<Vec<u8>>();

    std::thread::spawn(move || {
        while let Ok(bytes) = input_rx.recv() {
            if pty_writer.write_all(&bytes).is_err() {
                break;
            }
            let _ = pty_writer.flush();
        }
        debug!("pane writer thread exiting");
    });

    let pane = Arc::new(PaneState {
        pty: Mutex::new(pty),
        terminal: Mutex::new(terminal),
        input_tx,
    });

    Ok((pane, pty_reader))
}

fn spawn_reader_thread(
    pane_id: PaneId,
    pane: Arc<PaneState>,
    inner: Arc<SessionInner>,
    mut reader: PtyReader,
) {
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    pane.terminal.lock().unwrap().feed(&buf[..n]);
                    inner.dirty.store(true, Ordering::Release);
                }
            }
        }
        debug!(pane_id, "pane reader thread exiting");
    });
}
