#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

pub use wmux_core::{Dir, FocusDir};

// Bump on any change to a wire message. The client refuses to talk to a
// daemon whose PROTOCOL_VERSION it does not match.
// v3 (M4): ClientMsg::Command added.
pub const PROTOCOL_VERSION: u32 = 3;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct PtySize {
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Hello {
    pub client_version: String,
    pub protocol_version: u32,
    pub term_size: PtySize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Welcome {
    pub session_id: u64,
    pub protocol_version: u32,
}

// High-level session verbs the client can invoke (prefix keybindings).
// Lives alongside `Input`/`Resize` so we don't need a message variant per key.
#[derive(Debug, Serialize, Deserialize)]
pub enum Verb {
    SplitPane(Dir),
    FocusPane(FocusDir),
    KillPane,
}

// Messages the client sends to the daemon.
#[derive(Debug, Serialize, Deserialize)]
pub enum ClientMsg {
    Hello(Hello),
    Input(Vec<u8>),
    Resize(PtySize),
    Command(Verb),
    Bye,
}

// Messages the daemon sends to the client.
#[derive(Debug, Serialize, Deserialize)]
pub enum ServerMsg {
    Welcome(Welcome),
    // VT byte stream the client blits to stdout immediately after Welcome.
    // For multi-pane sessions this is the first composed frame.
    GridSnapshot(Vec<u8>),
    Output(Vec<u8>),
    ProtocolMismatch { server: u32, client: u32 },
    Bye(String),
}
