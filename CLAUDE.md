# CLAUDE.md — wmux

Guidance for Claude Code sessions working in this repo. Read before changing code.

## What this project is

A native Windows terminal multiplexer. Rust. Two binaries (`wmuxd`, `wmux`) in one Cargo workspace. See `SPEC.md` for the contract; see `progress.md` for where we actually are.

## Hard rules

1. **Windows only, natively.** No WSL, no Docker, no MSYS2, no Cygwin, no POSIX shims. If a dependency pulls in `nix`, `unix_socket`, or a build script that needs `bash`, replace it.
2. **ConPTY is the ground truth** for spawning shells. Don't reinvent pipes-into-cmd — use `CreatePseudoConsole` via the `windows` crate.
3. **No `unsafe` outside `wmux-conpty`.** That crate is the designated containment zone for Win32 FFI. Everywhere else, `#![forbid(unsafe_code)]`.
4. **Daemon never prints to its own stdout.** It runs headless; logs go through `tracing` to a file sink. `println!` in the daemon is a bug.
5. **Protocol changes bump `wmux-proto`'s version constant.** Client refuses mismatched daemons with a clear message rather than deserializing wrong bytes.
6. **Backwards-compat shims are banned until v0.1.0 ships.** Pre-release, just change the thing.

## How we work together

- **Let the user run shell commands unless they tell you otherwise.** Default to proposing the exact command (in a fenced block) and letting them paste or `!`-run it, rather than firing it yourself via Bash/PowerShell. File edits via Read/Write/Edit are fine; it's shell execution that defers to them.
- **Every new feature or milestone starts on its own branch.** Before writing code for it, confirm the branch name and ask the user to create it (`git checkout -b <name>` or similar). Naming convention: `mN-<slug>` for milestone work (e.g. `m0-scaffold`), `feat/<slug>` for other features.

## House style

- `rustfmt` defaults. `cargo fmt --all` before committing.
- `cargo clippy --workspace --all-targets -- -D warnings` must be clean.
- Error types: `thiserror` for library crates, `anyhow` only in the two binary crates (`wmuxd`, `wmux`).
- Panics only on genuinely unreachable invariants. Everything crossing an IPC boundary returns `Result`.
- Tests sit next to the code (`mod tests`) unless they're cross-crate, in which case `tests/`.
- Comments: default to none. Write one when the *why* is not obvious from the code — a Win32 flag choice, a workaround for a known ConPTY quirk, a race we deliberately allow.

## Code you'll touch often

| Crate | What lives here |
|---|---|
| `wmux-core`   | Pane tree, `Grid<Cell>`, `vte` parser integration, config structs. Pure logic, no I/O. |
| `wmux-proto`  | Serde types for every IPC message. Shared vocabulary. |
| `wmux-conpty` | `Pty::spawn`, `Pty::resize`, `Pty::reader`/`writer`. Only crate with `unsafe`. |
| `wmux-ipc`    | Named-pipe server + client, length-prefixed MessagePack framing. |
| `wmuxd`       | `main`, session registry, event loop, lifecycle. |
| `wmux`        | `main`, raw-mode TTY setup, input parser, renderer, status bar. |

## Running the thing

```powershell
# Build everything
cargo build --workspace

# Dev loop: run the daemon in one terminal with debug logs
$env:RUST_LOG = "wmuxd=debug,wmux_core=debug"
cargo run -p wmuxd -- --foreground

# In another terminal, attach
cargo run -p wmux -- attach
```

Tests:

```powershell
cargo test --workspace
```

## Things to watch out for

- **Line-ending auto-translation.** `ENABLE_PROCESSED_OUTPUT` + `ENABLE_WRAP_AT_EOL_OUTPUT` on the console mode can silently rewrite bytes. On the client we set `DISABLE_NEWLINE_AUTO_RETURN` and emit `\r\n` ourselves.
- **ConPTY writes partial frames.** Always drain the read pipe into a growable buffer before feeding the VT parser — sequences split across reads are common.
- **Pipe handles are not `Send` for free.** We wrap them in a type that owns the HANDLE and implements `Send + Sync` deliberately in `wmux-ipc`. Don't re-derive it elsewhere.
- **Resize storms.** A user dragging their terminal border fires `WINDOW_BUFFER_SIZE_EVENT` hundreds of times per second. Debounce (16 ms) before calling `ResizePseudoConsole`.
- **The daemon keeps running after the last client leaves** when `detach_keep_alive = true` (default). Killing a dev iteration means `taskkill /IM wmuxd.exe /F` or `wmux kill-server`.
- **`cmd.exe` emits VT only after `ENABLE_VIRTUAL_TERMINAL_PROCESSING` is set on its output handle.** ConPTY handles this for us — but if you ever bypass ConPTY for a diagnostic, remember it.

## Decisions already made (don't re-litigate)

- Rust, not C#/C++/Go. Chosen for memory safety + the `windows` crate + prior art (WezTerm, Zellij).
- Named pipes, not TCP, for IPC.
- MessagePack framing, not JSON. Compactness and schema evolution.
- TOML config, not YAML or JSON5.
- Server-side virtual grid, not blind byte forwarding. See SPEC §4.2.

## When in doubt

- Check `SPEC.md` first. If the spec doesn't answer it, propose a diff to the spec in the same PR that adds the behavior.
- Append a dated entry to `progress.md` for every notable change — milestones, blockers resolved, decisions reversed.
- A clarifying question is cheaper than a wrong refactor. Ask.
