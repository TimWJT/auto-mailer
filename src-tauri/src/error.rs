use serde::Serialize;

/// Every error the UI can see. The message is shown to the user as-is, so it
/// must be plain English, say what to do next, and never contain a password,
/// token or email body.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    User(String),
    #[error("Database problem: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("File problem: {0}")]
    Io(#[from] std::io::Error),
    #[error("Not found: {0}")]
    NotFound(String),
}

pub type AppResult<T> = Result<T, AppError>;

impl AppError {
    pub fn user(msg: impl Into<String>) -> Self {
        AppError::User(msg.into())
    }
}

impl Serialize for AppError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl From<serde_json::Error> for AppError {
    fn from(e: serde_json::Error) -> Self {
        AppError::User(format!("Couldn't read stored data: {e}"))
    }
}
