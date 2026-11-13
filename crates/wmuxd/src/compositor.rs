use std::io::Write as _;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use wmux_core::Rect;

use crate::session::SessionInner;

// Target refresh cadence. 16 ms ≈ 60 Hz. We throttle: no composition happens
// unless the dirty flag is set, so an idle session costs ~nothing.
const FRAME_INTERVAL: Duration = Duration::from_millis(16);

pub async fn run(inner: Arc<SessionInner>) {
    let mut ticker = tokio::time::interval(FRAME_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        ticker.tick().await;
        if inner.dead.load(Ordering::Acquire) {
            break;
        }
        if !inner.dirty.swap(false, Ordering::AcqRel) {
            continue;
        }

        let frame = compose(&inner);
        let client = inner.active_client.lock().unwrap().clone();
        if let Some(tx) = client {
            let _ = tx.send(frame);
        }
    }
}

pub fn compose(inner: &SessionInner) -> Vec<u8> {
    let state = inner.state.lock().unwrap();
    let size = state.size;
    let area = Rect {
        x: 0,
        y: 0,
        w: size.cols,
        h: size.rows,
    };
    let rects = state.tree.compute_rects(area);
    let borders = state.tree.collect_borders(area);
    let focused = state.focused;

    let mut out: Vec<u8> = Vec::with_capacity((size.cols as usize) * (size.rows as usize) * 4);
    // Hide cursor, reset SGR, clear screen, cursor home.
    out.extend_from_slice(b"\x1b[?25l\x1b[0m\x1b[2J\x1b[H");

    for (pane_id, rect) in &rects {
        let Some(pane) = state.panes.get(pane_id) else {
            continue;
        };
        let terminal = pane.terminal.lock().unwrap();
        emit_pane(&mut out, terminal.screen(), *rect);
    }

    for seg in &borders {
        emit_border(&mut out, seg);
    }

    // Place the real cursor at the focused pane's cursor.
    if let Some((_, rect)) = rects.iter().find(|(id, _)| *id == focused) {
        if let Some(pane) = state.panes.get(&focused) {
            let terminal = pane.terminal.lock().unwrap();
            let screen = terminal.screen();
            let (crow, ccol) = screen.cursor_position();
            let abs_x = rect.x + ccol.min(rect.w.saturating_sub(1));
            let abs_y = rect.y + crow.min(rect.h.saturating_sub(1));
            let _ = write!(out, "\x1b[{};{}H", abs_y as u32 + 1, abs_x as u32 + 1);
            if !screen.hide_cursor() {
                out.extend_from_slice(b"\x1b[?25h");
            }
        }
    }

    out
}

fn emit_pane(out: &mut Vec<u8>, screen: &vt100::Screen, rect: Rect) {
    let (screen_rows, screen_cols) = screen.size();
    let rows = (rect.h as usize).min(screen_rows as usize);
    let cols = (rect.w as usize).min(screen_cols as usize);

    for row in 0..rows {
        let _ = write!(
            out,
            "\x1b[{};{}H",
            rect.y as u32 + row as u32 + 1,
            rect.x as u32 + 1
        );
        out.extend_from_slice(b"\x1b[0m");
        for col in 0..cols {
            if let Some(cell) = screen.cell(row as u16, col as u16) {
                emit_cell_sgr(out, cell);
                let contents = cell.contents();
                if contents.is_empty() {
                    out.push(b' ');
                } else {
                    out.extend_from_slice(contents.as_bytes());
                }
            } else {
                out.push(b' ');
            }
        }
        out.extend_from_slice(b"\x1b[0m");
    }
}

fn emit_cell_sgr(out: &mut Vec<u8>, cell: &vt100::Cell) {
    // Build an SGR sequence from scratch each cell. Verbose on the wire but
    // trivially correct — no state to track.
    out.extend_from_slice(b"\x1b[0");
    if cell.bold() {
        out.extend_from_slice(b";1");
    }
    if cell.italic() {
        out.extend_from_slice(b";3");
    }
    if cell.underline() {
        out.extend_from_slice(b";4");
    }
    if cell.inverse() {
        out.extend_from_slice(b";7");
    }
    emit_color(out, cell.fgcolor(), true);
    emit_color(out, cell.bgcolor(), false);
    out.push(b'm');
}

fn emit_color(out: &mut Vec<u8>, color: vt100::Color, fg: bool) {
    let (base, bright, idx256, rgb) = if fg {
        (30u16, 90u16, "38;5;", "38;2;")
    } else {
        (40, 100, "48;5;", "48;2;")
    };
    match color {
        vt100::Color::Default => {}
        vt100::Color::Idx(n) if n < 8 => {
            let _ = write!(out, ";{}", base + n as u16);
        }
        vt100::Color::Idx(n) if n < 16 => {
            let _ = write!(out, ";{}", bright + (n as u16 - 8));
        }
        vt100::Color::Idx(n) => {
            let _ = write!(out, ";{}{}", idx256, n);
        }
        vt100::Color::Rgb(r, g, b) => {
            let _ = write!(out, ";{}{};{};{}", rgb, r, g, b);
        }
    }
}

fn emit_border(out: &mut Vec<u8>, seg: &wmux_core::BorderSeg) {
    match seg {
        wmux_core::BorderSeg::Vertical { x, y, h } => {
            for row in 0..*h {
                let _ = write!(
                    out,
                    "\x1b[{};{}H\u{2502}",
                    *y as u32 + row as u32 + 1,
                    *x as u32 + 1
                );
            }
        }
        wmux_core::BorderSeg::Horizontal { x, y, w } => {
            let _ = write!(out, "\x1b[{};{}H", *y as u32 + 1, *x as u32 + 1);
            for _ in 0..*w {
                out.extend_from_slice("\u{2500}".as_bytes());
            }
        }
    }
}
