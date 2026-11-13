#![forbid(unsafe_code)]

// Bump on any change to a wire message. The client refuses to talk to a
// daemon whose PROTOCOL_VERSION it does not match.
pub const PROTOCOL_VERSION: u32 = 1;
