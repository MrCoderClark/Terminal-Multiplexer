# Progress

Running log of what's done, what's in flight, and what's blocked. Newest entries on top.

## Status at a glance

- **Current milestone:** M4 — splits (code written, awaiting first interactive multi-pane test).
- **Last shipped:** M3 — virtual grid.
- **Blocked on:** nothing.

## Milestone checklist

- [ ] **M0 — scaffold.** Cargo workspace, 6 crate skeletons, CI green, `wmux --version` prints.
- [ ] **M1 — ConPTY round-trip.** `wmux-conpty` spawns `cmd.exe`, bytes flow both ways, `Pty::resize` works.
- [ ] **M2 — one pane end-to-end.** Daemon + client, one session, one pane. Named-pipe IPC. Dumb byte forwarding.
- [ ] **M3 — virtual grid.** `vte` parser integrated, per-pane `Grid<Cell>` maintained server-side, client renders from grid snapshots. Reattach redraws correctly.
- [ ] **M4 — splits.** Vertical + horizontal splits. Focus movement. Pane resize via drag or keybind.
- [ ] **M5 — tabs + status + copy mode (basic).** Tabs (windows), bottom status line, `prefix + [` scrollback view with copy.
- [ ] **M6 — config + keybindings.** `config.toml` loaded from `%APPDATA%\wmux\`, binding table honored, `source-file` reloads.
- [ ] **M7 — session CLI.** `wmux ls`, `attach`, `detach`, `new-session`, `kill-session`, `send-keys`.
- [ ] **M8 — scrollback + clipboard + doctor.** Full scrollback buffer, clipboard integration via `SetClipboardData`, `wmux doctor`.
- [ ] **M9 — docs + installer + v0.1.0.** README, zipped release, optional MSIX, tag.

## Log

### 2026-11-13 — M4 splits + compositor
- Branch `m4-splits`. No new workspace deps (vt100/tokio already there).
- `wmux-core` grew a `pane_tree` module alongside `terminal`. `PaneTree = Leaf(PaneId) | Split { dir, ratio, a, b }` with serde derives. `Dir` (H/V), `FocusDir` (Left/Right/Up/Down), and `Rect` live here too. Methods: `contains`, `all_pane_ids`, `split_leaf`, `remove` (refuses the last leaf), `compute_rects` (walks the tree with a 1-column/row border reserved per split), `collect_borders` (returns `BorderSeg::Vertical`/`Horizontal` for rendering), and `focus_from(current, dir, area)` (first adjacent rect in tree order with orthogonal overlap wins). Unit tests cover single-leaf, split with border accounting, remove-promotes-sibling, remove-last-fails, and focus-right/left.
- `wmux-proto` gets `Verb { SplitPane(Dir), FocusPane(FocusDir), KillPane }` and `ClientMsg::Command(Verb)`. `Dir` / `FocusDir` are re-exported from `wmux-core` to keep one definition. `PROTOCOL_VERSION` bumped to `3` per CLAUDE.md rule #5.
- `wmuxd` is reorganised into `main.rs` + `session.rs` + `compositor.rs`. `SessionState` holds the `PaneTree`, a `HashMap<PaneId, Arc<PaneState>>`, the focused pane id, and the current terminal size. Each `PaneState` owns its own `Mutex<Pty>` + `Mutex<Terminal>` + input mpsc. `Session::spawn` boots an initial pane and starts the compositor task; `Session::split_focused` adds a new ConPTY sized to the new pane's rect and resizes the sibling, `focus_move` nudges `focused`, `kill_focused` removes the pane and promotes the sibling (killing the last pane marks the session dead). Every mutation flips the `dirty: AtomicBool`.
- Compositor runs on tokio at up to 60 Hz. On each tick: if `dirty` was set, build one composed frame — hide cursor + reset SGR + clear screen, walk each pane rect emitting per-cell SGR (bold/italic/underline/inverse + 8/16/256/truecolor fg/bg) and cell contents, draw box-drawing border segments (U+2502 vertical, U+2500 horizontal), then place the real terminal cursor at the focused pane's cursor position and show it. The entire frame ships as a single `ServerMsg::Output`.
- `wmux` extends the Ctrl+A prefix handler: `"` = vsplit (new pane below, Dir::V), `%` = hsplit (new pane right, Dir::H), `h`/`j`/`k`/`l` = focus left/down/up/right, `x` = kill focused pane, `d` = detach (unchanged). Unknown keys after Ctrl+A still forward the Ctrl+A + the key, as before. New `StdinEvent::Command(Verb)` variant routes verbs to the pipe as `ClientMsg::Command` frames.
- Known rough edges carried over: no resize-while-attached, no zoom, no mouse, no per-pane title; the compositor does a full redraw every frame (no cell-level diff).

### 2026-11-13 — M3 virtual grid + persistent session + snapshot on attach
- Branch `m3-virtual-grid`. Workspace deps grew `vt100 = "0.15"`.
- `wmux-core` now exposes `Terminal`: a thin wrapper around `vt100::Parser` with `feed`, `resize`, `snapshot`, `size`. `vt100` bundles `vte` plus a real grid, cursor, and modes, so we got Alacritty-lineage VT emulation without hand-rolling.
- `wmux-proto` adds `ServerMsg::GridSnapshot(Vec<u8>)` (VT bytes the client blits to stdout on attach) and bumps `PROTOCOL_VERSION` to `2` per CLAUDE.md rule #5.
- `wmuxd` refactored into a persistent `Session` (owns `Pty`, `Terminal`, input/output threads) held in `DaemonState.session: AsyncMutex<Option<Session>>`. The PTY reader thread feeds the terminal on every chunk and, when a client is attached, also forwards the chunk to that client's `mpsc::UnboundedSender<Vec<u8>>`. On client disconnect the session stays alive — `detach()` just clears the `active_client` slot. When the PTY dies (`ReadFile` returns 0 / error), the reader thread flags the session dead; the next `Hello` sees `is_dead()` and spawns a fresh one.
- `Session::attach()` returns `(output_rx, snapshot_bytes)` in one shot; the daemon sends `Welcome` then immediately pushes `ServerMsg::GridSnapshot(snapshot_bytes)` before entering the forward loop. `Session::resize()` resyncs the ConPTY and the virtual grid together so a reattaching client at a different size doesn't desync.
- Client reads `Welcome`, then `GridSnapshot`, enters raw mode, writes the snapshot bytes to stdout, prints the detach banner, and enters the normal select loop. Running `wmux attach` after Ctrl+A d now brings up the shell with the same cursor / screen as the detached session.
- Still deferred: resize-while-attached (pairs with M4's splits), scrollback (M8), and surfacing a way to kill a stuck session (M7).

### 2026-11-13 — M2 single-pane daemon + client written
- Branch `m2-one-pane-end-to-end`. Workspace deps grew `tokio` (rt-multi-thread, macros, net, io-util, io-std, sync, time), `rmp-serde = "1.3"`, `crossterm = "0.28"`.
- `wmux-proto` now carries real wire types: `Hello`, `Welcome`, `PtySize`, and two message enums `ClientMsg { Hello, Input, Resize, Bye }` / `ServerMsg { Welcome, Output, ProtocolMismatch, Bye }`. Everything is serde-derived; `PROTOCOL_VERSION` stays at `1`.
- `wmux-ipc` provides length-prefixed MessagePack framing (`write_frame` / `read_frame`, 4-byte big-endian length, 16 MiB payload cap) and the shared pipe path `\\.\pipe\wmux`. Per-user SID suffix (SPEC §4.3) is deferred — single local wmux assumed for M2.
- `wmux-conpty` now has `unsafe impl Send for Pty` with a safety comment: HPCON/attribute-list pointers are opaque, and ConPTY's own APIs are documented thread-safe when callers serialize access.
- `wmuxd` runs a tokio multi-thread runtime. One client at a time: it accepts on the pipe, reads the `Hello`, version-checks, spawns `cmd.exe` through `wmux-conpty`, sends `Welcome`, then runs three concurrent movers — an OS thread pumping `PtyReader` → `mpsc::Sender<Vec<u8>>`, an OS thread pumping `std::sync::mpsc::Receiver<Vec<u8>>` → `PtyWriter`, and a tokio task forwarding the output channel to `ServerMsg::Output` frames. The main client task reads `ClientMsg` frames and dispatches (`Input`, `Resize`, `Bye`). On disconnect it drops the input channel, joins the writer thread, drops the `Pty` (closes ConPTY), joins the reader thread, and loops back to serve the next client.
- `wmux attach` opens the pipe, sends `Hello` with the current terminal size (via `crossterm::terminal::size`), handles the `Welcome` / `ProtocolMismatch` fork, enters raw mode (guarded by a `RawModeGuard` that restores on any exit), spawns an OS thread reading stdin and feeding an mpsc, and uses `tokio::select!` to multiplex pipe reads (→ stdout) and stdin chunks (→ `ClientMsg::Input` frames). No resize-while-attached yet; terminal size is a one-shot at handshake.
- Shell is hardcoded to `cmd.exe` — configurable default lands in M6.

### 2026-11-13 — M1 ConPTY wrapper written
- Branch `m1-conpty-round-trip`. `wmux-conpty` now pulls in `windows = "0.58"` with the Console / Pipes / Threading / Foundation / Security feature set.
- `Pty::spawn(command_line, PtySize)` wires up the full ConPTY setup: `CreatePipe` twice, `CreatePseudoConsole`, then an attribute list built via the two-call `InitializeProcThreadAttributeList` dance plus `UpdateProcThreadAttribute(PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE)`, and finally `CreateProcessW` with `EXTENDED_STARTUPINFO_PRESENT`. Pipe HANDLEs are wrapped in `std::fs::File` via `FromRawHandle` so the returned `PtyReader` / `PtyWriter` are plain `Read`/`Write` and inherit `Send`.
- `Pty::resize`, `Pty::wait(timeout)`, `Pty::pid()` round out the surface. `Drop` calls `ClosePseudoConsole` + `DeleteProcThreadAttributeList`; the child process handle is an `OwnedHandle` so it closes itself.
- Attribute-list backing storage is held in a `Vec<u8>` field on `Pty` (`_attr_list_buf`) so it outlives the HPCON — the raw pointer stored in `attr_list` is read during `ClosePseudoConsole`.
- `PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE` is hardcoded as `0x00020016` to insulate the crate from `windows` crate version churn. Documented inline.
- Example `cargo run -p wmux-conpty --example cmd_echo` spawns `cmd.exe /c echo hello from ConPTY`, prints raw PTY bytes. First visible sign of life.
- Integration test `crates/wmux-conpty/tests/echo.rs` asserts a known token survives the round-trip. Runs under `cargo test --workspace`.

### 2026-11-13 — M0 scaffold laid down
- Branch `m0-scaffold`. Workspace `Cargo.toml` with `resolver = "2"`, `members = ["crates/*"]`, shared `workspace.package` (version 0.0.1, edition 2021, rust 1.75) and `workspace.dependencies` for `anyhow`, `clap`, `thiserror`, `serde`, `tracing`, `tracing-subscriber`.
- Six crate skeletons created: `wmux-core`, `wmux-proto`, `wmux-conpty`, `wmux-ipc`, `wmuxd`, `wmux`. Every crate forbids `unsafe` except `wmux-conpty`, which carries a `compile_error!` guard on non-Windows targets so cross-building fails loudly.
- `wmux-proto` defines `PROTOCOL_VERSION: u32 = 1` — the constant CLAUDE.md pins as the version the client enforces.
- `wmuxd` main parses `--foreground`, wires `tracing-subscriber` to **stderr** (never stdout, per CLAUDE.md rule 4), and uses `EnvFilter` so `RUST_LOG=wmuxd=debug` works.
- `wmux` main is a bare clap derive with no subcommands yet — enough to satisfy the M0 acceptance criterion `wmux --version`. Subcommand plumbing lands in M7.
- `rust-toolchain.toml` pins stable + rustfmt + clippy.
- `.github/workflows/ci.yml` runs on `windows-latest`: fmt check, clippy with `-D warnings`, `cargo test --workspace`, and executes `cargo run -p wmux -- --version` as a smoke check.
- `examples/config.toml` lifted verbatim from SPEC §6.
- CLAUDE.md gained a **How we work together** section codifying two rules: the user runs shell commands (I propose, they execute) and every new feature/milestone opens its own branch (`mN-<slug>` / `feat/<slug>`).

### 2026-11-13 — project kickoff
- Decided on Rust for the implementation (memory safety, strong `windows` crate, prior art in WezTerm / Alacritty / Zellij).
- Wrote `SPEC.md`: architecture (daemon + client over named pipe), ConPTY per pane, server-side virtual grid, TOML config, 9-milestone plan.
- Wrote `CLAUDE.md`: house rules — Windows-native only, `unsafe` confined to `wmux-conpty`, no `println!` in the daemon, bump `wmux-proto` version on protocol changes.
- No code yet. Next step: run `cargo new --lib` for each crate and wire up the workspace `Cargo.toml`.

## Open questions

1. Host-terminal compatibility matrix for v1 — commit to Windows Terminal as baseline; Alacritty / WezTerm / VS Code / ConEmu as nice-to-have. Needs a test pass before v0.1.0.
2. CLI-only for v1, or ship a GUI launcher too? Leaning CLI-only.
3. Which `Ctrl`-combos do we reserve vs forward? Depends on host terminal quirks; defer until M5.
4. Should the daemon survive a reboot via a Scheduled Task? Post-v1 call.

## Decisions reversed / things we tried and dropped

*(none yet)*
