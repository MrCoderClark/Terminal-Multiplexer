#![forbid(unsafe_code)]

use std::io;

use serde::{de::DeserializeOwned, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

// Named-pipe path used by the daemon and the client. The SID-suffixed form
// described in SPEC §4.3 (one pipe per local user) lands in a later
// milestone — for M2 we trust that only one wmux runs per Windows session.
pub const PIPE_NAME: &str = r"\\.\pipe\wmux";

// Hard cap on a single frame's payload. Guards against a bad peer trying
// to make us allocate a multi-GB buffer.
pub const MAX_FRAME_BYTES: u32 = 16 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    #[error("MessagePack encode error: {0}")]
    Encode(#[from] rmp_serde::encode::Error),

    #[error("MessagePack decode error: {0}")]
    Decode(#[from] rmp_serde::decode::Error),

    #[error("frame too large: {0} bytes (max {MAX_FRAME_BYTES})")]
    FrameTooLarge(u32),
}

pub type Result<T> = std::result::Result<T, Error>;

// Length-prefixed MessagePack frame: 4-byte big-endian length, then payload.
pub async fn write_frame<T, W>(w: &mut W, msg: &T) -> Result<()>
where
    T: Serialize,
    W: AsyncWriteExt + Unpin,
{
    let bytes = rmp_serde::to_vec(msg)?;
    let len = u32::try_from(bytes.len()).map_err(|_| Error::FrameTooLarge(u32::MAX))?;
    if len > MAX_FRAME_BYTES {
        return Err(Error::FrameTooLarge(len));
    }
    w.write_all(&len.to_be_bytes()).await?;
    w.write_all(&bytes).await?;
    w.flush().await?;
    Ok(())
}

pub async fn read_frame<T, R>(r: &mut R) -> Result<T>
where
    T: DeserializeOwned,
    R: AsyncReadExt + Unpin,
{
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf);
    if len > MAX_FRAME_BYTES {
        return Err(Error::FrameTooLarge(len));
    }
    let mut buf = vec![0u8; len as usize];
    r.read_exact(&mut buf).await?;
    Ok(rmp_serde::from_slice(&buf)?)
}
