# wmux — Windows Terminal Multiplexer

A native Windows terminal multiplexer in the spirit of tmux and Zellij. No WSL. No Docker. No MSYS2. Just Win32 + ConPTY + Rust.

## 1. Goals

- Run multiple shell sessions inside one terminal window via **panes** (splits) and **tabs**.
- **Detach** a session (keep processes alive) and **re-attach** from a new terminal window later.
- Fast, native, no dependency on a POSIX layer.
- Config file + keybindings in the tmux/zellij tradition, but Windows-idiomatic defaults.
- Work inside any modern Windows terminal emulator that supports VT sequences (Windows Terminal, Alacritty, WezTerm, ConEmu, VS Code integrated terminal).

## 2. Non-goals (for v1)

- POSIX compatibility layer or MSYS/Cygwin builds.
- Remote/SSH session sharing — a session is pinned to the local host.
- GPU-accelerated glyph rendering — we are a multiplexer, not a terminal emulator. The host terminal renders glyphs.
- Mouse-first UX. Mouse support is a stretch goal; keyboard is the contract.
- Scripting language or plugin runtime (post-v1).

## 3. Platform requirements

- Windows 10 1809 (build 17763) or newer — required for ConPTY (`CreatePseudoConsole`).
- Windows 11 is the primary dev target.
- x64 only for v1. ARM64 is a stretch.
- Host terminal must process ANSI/VT sequences (true for Windows Terminal, Alacritty, WezTerm, VS Code, modern ConEmu).

## 4. Architecture

Two binaries from one Rust workspace:

| Binary | Role |
|---|---|
| `wmuxd.exe` | Daemon / server. Owns sessions, panes, ConPTY handles, child processes. Exposes a named-pipe RPC. Headless. |
| `wmux.exe`  | Client / front-end. Attaches to the daemon over a named pipe, forwards host-terminal input, draws pane content, handles keybindings and the status line. |

The split mirrors tmux (`tmux` server + `tmux` client). It is what makes detach/attach possible: closing the client window never kills child processes because they live under the daemon, not the client's console.

### 4.1 Processes, consoles, and ConPTY

Each pane owns one ConPTY instance:

```
wmuxd ──spawns──► child shell (cmd / pwsh / nu / …)
   │                 ▲
   │ CreatePseudoConsole creates a pair of
   │ anonymous pipes: (input_write, output_read)
   │ for the daemon, (input_read, output_write)
   │ wired into the child's StdIn/StdOut/StdErr.
   │
   └── Daemon writes keystrokes to input_write,
       reads VT-encoded screen updates from output_read.
```

Key Win32 calls:
- `CreatePseudoConsole(size, hInput, hOutput, flags, &hPC)`
- `InitializeProcThreadAttributeList` + `UpdateProcThreadAttribute` with `PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE`
- `CreateProcessW` with `EXTENDED_STARTUPINFO_PRESENT`
- `ResizePseudoConsole(hPC, newSize)` when the pane resizes
- `ClosePseudoConsole(hPC)` on teardown

Crates we'll lean on (all pure-Rust, no cygwin):
- `windows` (Microsoft's official Win32 bindings) — ConPTY, pipes, process APIs
- `crossterm` — client-side raw mode, input parsing, VT output on stdout
- `vte` or `anstyle-parse` — parse the stream coming back from ConPTY so we can maintain a virtual grid per pane
- `serde` + `toml` — config
- `tokio` or hand-rolled OVERLAPPED I/O — async for pipes and ConPTY streams
- `tracing` + `tracing-subscriber` — structured logs

### 4.2 Virtual grid per pane

Each pane keeps a server-side grid (rows × cols of `Cell { ch, fg, bg, attr }`) fed by a VT parser consuming ConPTY output. The client renders only the slice it owns. This matters because:

1. Reattach has to redraw the full screen — the daemon must know the current cell state without replaying history.
2. Panes smaller than what the child thinks the terminal is would corrupt rendering if we blindly forwarded bytes.
3. Scrollback belongs to the daemon.

### 4.3 Client ↔ daemon IPC

Transport: **Windows named pipe**, `\\.\pipe\wmux-<user-sid>`. One pipe per session attachment. Length-prefixed MessagePack frames (via `rmp-serde`). Why not TCP: we don't want a listener on any port, and named pipes carry the user SID for access checks.

Message categories:
- `Hello { client_version, term_size, env }` → `Welcome { session_id, protocol_version }`
- `Resize { session_id, cols, rows }`
- `Input { session_id, pane_id, bytes }` — raw keyboard input to the focused pane
- `Command { session_id, verb }` — split, focus, new-tab, detach, kill-pane, …
- `Output { pane_id, patch }` — grid deltas pushed from daemon to client
- `PaneLayout { session_id, tree }` — tree of splits/tabs with sizes
- `StatusUpdate { fields }` — status-line contents
- `Bye { reason }`

Client renders at 60 fps max; daemon coalesces output patches.

### 4.4 Pane layout tree

```
Session
  └── Window (a.k.a. tab)
       └── PaneLayout = Leaf(pane_id) | Split { dir, ratio, a, b }
```

- `dir` is Horizontal or Vertical.
- `ratio` is a float in (0, 1) with a minimum column/row clamp.
- Resizing a split adjusts `ratio` and cascades a `ResizePseudoConsole` to every affected pane.

### 4.5 Detach / attach

- `wmux detach` — client sends `Bye { reason: UserDetach }`, closes the pipe, exits. Daemon keeps running.
- `wmux attach [session]` — client connects, sends `Hello`, daemon replies with full `PaneLayout` + current grid snapshot for each visible pane. Client redraws from snapshot and then applies streaming patches.
- `wmux ls` — list sessions via a transient control-pipe command.

### 4.6 Daemon lifecycle

- First `wmux` invocation auto-spawns `wmuxd.exe` if no pipe answers.
- Daemon exits when the last session is killed **and** `detach_keep_alive = false` (configurable; default `true`).
- Crash recovery: daemon writes a small session-index file to `%LOCALAPPDATA%\wmux\sessions.json` on every layout change so a restart can at least surface which sessions were lost.

## 5. Commands and keybindings

Prefix key (default): `Ctrl+A`. Configurable.

| Keybinding | Action |
|---|---|
| `prefix` + `"`                 | Split pane vertically (new pane below) |
| `prefix` + `%`                 | Split pane horizontally (new pane right) |
| `prefix` + arrow               | Focus adjacent pane |
| `prefix` + `z`                 | Toggle zoom on focused pane |
| `prefix` + `x`                 | Kill focused pane |
| `prefix` + `c`                 | New tab (window) |
| `prefix` + `n` / `p`           | Next / previous tab |
| `prefix` + `1`..`9`            | Jump to tab N |
| `prefix` + `d`                 | Detach session |
| `prefix` + `[`                 | Enter copy / scroll mode |
| `prefix` + `:`                 | Command prompt |
| `prefix` + `?`                 | Keybinding cheatsheet overlay |

CLI verbs: `wmux new-session [-s name] [cmd…]`, `wmux attach [name]`, `wmux ls`, `wmux kill-session <name>`, `wmux send-keys <target> <keys>`, `wmux split-window`, `wmux list-panes`, `wmux source-file <path>`.

## 6. Config file

- Path: `%APPDATA%\wmux\config.toml` (override with `WMUX_CONFIG`).
- Format: TOML. Reasoning: no whitespace footguns, no external quirks, first-class in the Rust ecosystem.

```toml
# Example config
default_shell = "pwsh.exe"           # or "cmd.exe", or an absolute path
default_shell_args = ["-NoLogo"]
prefix = "Ctrl+A"
scrollback_lines = 10000
mouse = true
status_interval_ms = 1000
detach_keep_alive = true

[status_line]
left  = "#{session_name} | #{window_index}:#{window_name}"
right = "#{cpu}% | #{mem_mb}MB | %H:%M"

[[binding]]
key    = "prefix v"
action = "split-pane --vertical"

[[binding]]
key    = "prefix h"
action = "split-pane --horizontal"

[colors]
status_fg = "#e6e6e6"
status_bg = "#303030"
border_active   = "#5fafff"
border_inactive = "#404040"
```

## 7. Status line

Rendered by the client in the bottom row (or top, configurable). Format-string tokens expand against daemon state pushed in `StatusUpdate`:

- `#{session_name}`, `#{window_index}`, `#{window_name}`, `#{pane_title}`
- `#{host}`, `#{user}`
- `#{cpu}`, `#{mem_mb}` — polled from the daemon via `GetSystemTimes` / `GlobalMemoryStatusEx`
- Any token prefixed `shell:` runs a configured shell snippet — stretch goal.

## 8. Rendering, input, and VT handling

### 8.1 Client-side rendering

- Enable virtual terminal processing on the client's stdout with `SetConsoleMode(ENABLE_VIRTUAL_TERMINAL_PROCESSING | DISABLE_NEWLINE_AUTO_RETURN)`.
- Switch to the alternate screen on attach, restore on detach.
- Diff the current frame against the previous one; emit minimal cursor-position + attribute-change + text runs. (`crossterm::queue!` + a backbuffer.)
- Enforce a max frame rate (default 60 Hz) to avoid starving a slow host terminal.

### 8.2 Client-side input

- Raw mode on stdin.
- Parse keys through `crossterm::event` (handles VT key sequences and Win32 input records).
- A pressed key is matched against the keybinding table first; on miss, it is forwarded to the focused pane as bytes.
- Special care for `Ctrl` combos that Windows terminals historically eat (`Ctrl+Space`, `Ctrl+@`) — document which are usable per host terminal.

### 8.3 Daemon-side VT parsing

- Parser: `vte` (the same state machine Alacritty uses).
- Maintain per-pane state: cursor x/y, SGR, scroll region, DEC modes we care about (origin mode, autowrap, alt screen), tab stops.
- Alt screen is tracked separately so switching back restores the primary buffer without a replay.
- Unknown DCS/OSC sequences are logged at trace level and dropped.

## 9. Scrollback and copy mode

- Each pane keeps `scrollback_lines` wrapped lines (default 10 000).
- Copy mode: `prefix + [` suspends live rendering, draws the scrollback window, lets the user scroll with arrow keys / PgUp / PgDn, select with `v` + motion, copy with `y`.
- Clipboard: `OpenClipboard` / `SetClipboardData(CF_UNICODETEXT, …)`.

## 10. Logging and diagnostics

- `tracing` with a rotating file sink at `%LOCALAPPDATA%\wmux\logs\wmuxd-YYYYMMDD.log`.
- `wmux --log-level debug` sets the daemon level via a control message.
- `wmux doctor` — prints Windows build, Rust toolchain, pipe reachability, ConPTY availability, config parse result.

## 11. Security model

- Pipe ACL: allow only the user SID and SYSTEM. Everything else denied. Build the SD with `ConvertStringSecurityDescriptorToSecurityDescriptorW("D:P(A;;GA;;;<userSID>)(A;;GA;;;SY)")` or the equivalent SDDL.
- No listening TCP socket. No remote attach.
- Daemon never reads the user's shell history or environment beyond what is needed to spawn the shell.
- Config file is parsed in a sandboxed `serde::Deserializer`; no arbitrary code execution surface.

## 12. Testing strategy

- Unit tests on the VT state machine (feed canned byte streams, assert the resulting grid). Reuse Alacritty's `vte` conformance cases where applicable.
- Pane-tree property tests with `proptest`: splits + closes + resizes should always yield a valid layout summing to window area.
- An integration harness that:
  1. Spawns `wmuxd.exe` into a temp `%LOCALAPPDATA%`.
  2. Connects a fake client over the pipe.
  3. Opens a pane running `cmd /c echo hello && exit`.
  4. Reads output frames and asserts "hello" lands in the right cell.
- End-to-end: launch `wmuxd` + `wmux attach` in a Windows Terminal profile, run a scripted session with `wmux send-keys`, snapshot the final grid, compare against golden file.
- CI: GitHub Actions `windows-latest` runner, `cargo test --workspace`.

## 13. Repository layout

```
wmux/
├── Cargo.toml                 # workspace
├── SPEC.md                    # this file
├── CLAUDE.md                  # dev guidance
├── progress.md                # progress log
├── README.md                  # user-facing (written later)
├── crates/
│   ├── wmux-core/             # layout tree, grid, VT parser, config
│   ├── wmux-proto/            # IPC message types (shared by client + daemon)
│   ├── wmux-conpty/           # thin safe wrapper around ConPTY + CreateProcess
│   ├── wmux-ipc/              # named-pipe server + client, framing
│   ├── wmuxd/                 # daemon binary
│   └── wmux/                  # client binary
├── tests/                     # cross-crate integration tests
└── examples/
    └── config.toml            # shipped example
```

## 14. Milestones

| M | Deliverable |
|---|---|
| M0 | Workspace scaffold, CI green, `wmux --version` prints. |
| M1 | `wmux-conpty` spawns `cmd.exe`, round-trips bytes, resize works. |
| M2 | Daemon with one session, one pane, no splits. Client attaches over named pipe, forwards input, renders a dumb byte stream. |
| M3 | VT parser + virtual grid + diff rendering. Reattach redraws correctly. |
| M4 | Vertical + horizontal splits. Focus and resize. |
| M5 | Tabs (windows), status line, copy mode (basic). |
| M6 | Config file, keybindings table, `source-file`. |
| M7 | `wmux ls` / `attach` / `detach` / `kill-session` CLI complete. |
| M8 | Scrollback, clipboard, `wmux doctor`. |
| M9 | Docs, installer (plain zip + optional MSIX), v0.1.0 tag. |

## 15. Open questions

1. Host-terminal compatibility matrix — which emulators do we commit to in v1? Baseline: Windows Terminal. Nice-to-have: Alacritty, WezTerm, VS Code, ConEmu.
2. Do we need a GUI launcher, or is CLI-only fine for v1? Assumption: CLI-only.
3. `Ctrl` key mapping differs across host terminals; which combos should we reserve vs forward? Needs a small compatibility test pass.
4. Should the daemon survive a reboot via a Scheduled Task entry? Probably post-v1.
