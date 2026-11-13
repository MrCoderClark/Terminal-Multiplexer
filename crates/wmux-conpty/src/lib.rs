// The one crate where `unsafe` lives — Win32 FFI containment zone.
// Every other crate in the workspace sets `#![forbid(unsafe_code)]`.

#[cfg(not(windows))]
compile_error!("wmux-conpty is Windows-only and relies on ConPTY (CreatePseudoConsole).");
