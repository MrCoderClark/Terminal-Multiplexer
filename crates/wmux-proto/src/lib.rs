#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

// Bump on any change to a wire message. The client refuses to talk to a
// daemon whose PROTOCOL_VERSION it does not match.
// v2 (M3): ServerMsg::GridSnapshot added.
pub const PROTOCOL_VERSION: u32 = 2;

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

// Messages the client sends to the daemon.
#[derive(Debug, Serialize, Deserialize)]
pub enum ClientMsg {
    Hello(Hello),
    Input(Vec<u8>),
    Resize(PtySize),
    Bye,
}

// Messages the daemon sends to the client.
#[derive(Debug, Serialize, Deserialize)]
pub enum ServerMsg {
    Welcome(Welcome),
    // VT byte stream sent right after Welcome that redraws the current
    // screen state. The client blits these bytes to stdout before entering
    // its main loop so reattach shows exactly what detach left behind.
    GridSnapshot(Vec<u8>),
    Output(Vec<u8>),
    ProtocolMismatch { server: u32, client: u32 },
    Bye(String),
}
