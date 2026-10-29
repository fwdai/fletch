use std::io;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: io::Error,
    },
    #[error("invalid VM spec: {0}")]
    InvalidSpec(String),
    #[error("boxes are unavailable here: {0}")]
    Unsupported(String),
    /// The guest did not reach `FLETCH_READY`; `console` is the tail of what it printed.
    #[error("{reason}\n--- console ---\n{console}")]
    NotReady { reason: String, console: String },
    #[error("{url}: {message}")]
    Http { url: String, message: String },
    /// The release's `manifest.json` is unreadable or not for this build's image.
    #[error("{url}: {message}")]
    Manifest { url: String, message: String },
    #[error("the download is {actual} bytes, not the {expected} its manifest lists")]
    Size { expected: u64, actual: u64 },
    #[error("the download's sha256 is not the one its manifest lists")]
    Sha256,
    #[error("cancelled")]
    Cancelled,
    #[error("the installed image {version} is damaged and was removed")]
    Damaged { version: String },
    #[error("{0}")]
    Other(String),
}

impl Error {
    pub(crate) fn io(context: impl Into<String>) -> impl FnOnce(io::Error) -> Error {
        let context = context.into();
        move |source| Error::Io { context, source }
    }
}

pub type Result<T> = std::result::Result<T, Error>;
