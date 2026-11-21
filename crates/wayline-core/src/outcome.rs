//! Stable transfer failure categories for every user interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferFailureKind {
    Declined,
    Cancelled,
    Interrupted,
    Connection,
    Validation,
    Other,
}

impl TransferFailureKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Declined => "declined",
            Self::Cancelled => "cancelled",
            Self::Interrupted => "interrupted",
            Self::Connection => "connection",
            Self::Validation => "validation",
            Self::Other => "other",
        }
    }

    pub(crate) fn from_io(error: &std::io::Error) -> Self {
        use std::io::ErrorKind::*;
        match error.kind() {
            ConnectionAborted | ConnectionReset | ConnectionRefused | NotConnected | BrokenPipe
            | UnexpectedEof | TimedOut | WouldBlock => Self::Connection,
            InvalidData => Self::Validation,
            _ => Self::Other,
        }
    }

    // These indicate the stream was closed or reset, without telling us why.
    // Timeouts and TLS/protocol failures retain their ordinary error category.
    pub(crate) fn is_stream_disconnected(error: &std::io::Error) -> bool {
        use std::io::ErrorKind::*;
        matches!(
            error.kind(),
            UnexpectedEof | ConnectionReset | ConnectionAborted | BrokenPipe | NotConnected
        )
    }
}
