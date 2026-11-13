// This is the one crate where `unsafe` lives — Win32 FFI containment zone.
// Every other crate in the workspace sets `#![forbid(unsafe_code)]`.

#[cfg(not(windows))]
compile_error!("wmux-conpty is Windows-only and relies on ConPTY (CreatePseudoConsole).");

#[cfg(windows)]
mod error;
#[cfg(windows)]
mod pty;

#[cfg(windows)]
pub use error::{Error, Result};
#[cfg(windows)]
pub use pty::{Pty, PtyReader, PtySize, PtyWriter};
