#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

// Bump on any change to a wire message. The client refuses to talk to a
// daemon whose PROTOCOL_VERSION it does not match.
pub const PROTOCOL_VERSION: u32 = 1;

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
    Output(Vec<u8>),
    ProtocolMismatch { server: u32, client: u32 },
    Bye(String),
}
