#![forbid(unsafe_code)]

// Server-side virtual terminal: VT parser + grid of cells. We lean on
// `vt100` (which internally uses `vte`, the same state machine Alacritty
// ships) so that this crate can stay focused on wmux-specific glue rather
// than re-implementing a VT emulator.

pub struct Terminal {
    parser: vt100::Parser,
}

impl Terminal {
    pub fn new(cols: u16, rows: u16) -> Self {
        // Scrollback = 0 for M3; proper scrollback is M8.
        Self {
            parser: vt100::Parser::new(rows, cols, 0),
        }
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.parser.set_size(rows, cols);
    }

    // VT byte stream that redraws the current screen state when written to
    // a terminal. This is what we send a reattaching client so its terminal
    // comes up showing exactly what the detached session looked like.
    pub fn snapshot(&self) -> Vec<u8> {
        self.parser.screen().contents_formatted()
    }

    pub fn size(&self) -> (u16, u16) {
        // vt100 reports (rows, cols); convert to (cols, rows) to match the
        // rest of the workspace.
        let (rows, cols) = self.parser.screen().size();
        (cols, rows)
    }
}
