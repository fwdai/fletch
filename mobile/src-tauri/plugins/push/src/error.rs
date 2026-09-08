use serde::{Serialize, Serializer};

pub type Result<T> = std::result::Result<T, Error>;

/// Every failure here ends up as a string in the webview — "push is iOS-only",
/// or whatever the Swift side rejected with — so the error is that string and
/// there is nothing to match on.
#[derive(Debug)]
pub struct Error(String);

impl Error {
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

impl Serialize for Error {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

#[cfg(target_os = "ios")]
impl From<tauri::plugin::mobile::PluginInvokeError> for Error {
    fn from(error: tauri::plugin::mobile::PluginInvokeError) -> Self {
        Self(error.to_string())
    }
}
