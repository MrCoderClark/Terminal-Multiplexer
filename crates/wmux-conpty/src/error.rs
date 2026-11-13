use std::io;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("CreatePipe failed: {0}")]
    CreatePipe(#[source] io::Error),

    #[error("CreatePseudoConsole failed: {0}")]
    CreatePseudoConsole(#[source] windows::core::Error),

    #[error("ResizePseudoConsole failed: {0}")]
    ResizePseudoConsole(#[source] windows::core::Error),

    #[error("InitializeProcThreadAttributeList failed: {0}")]
    InitAttributeList(#[source] windows::core::Error),

    #[error("UpdateProcThreadAttribute failed: {0}")]
    UpdateAttribute(#[source] windows::core::Error),

    #[error("CreateProcess failed: {0}")]
    CreateProcess(#[source] windows::core::Error),

    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
