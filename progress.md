# Progress

Running log of what's done, what's in flight, and what's blocked. Newest entries on top.

## Status at a glance

- **Current milestone:** M1 — ConPTY round-trip (code written, awaiting first `cargo test`).
- **Last shipped:** M0 scaffold.
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
