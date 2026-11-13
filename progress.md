# Progress

Running log of what's done, what's in flight, and what's blocked. Newest entries on top.

## Status at a glance

- **Current milestone:** M0 — workspace scaffold.
- **Last shipped:** nothing yet.
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
