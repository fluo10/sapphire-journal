use std::fmt;

#[derive(Debug)]
pub enum AppError {
    Io(std::io::Error),
    Toml(String),
    Git(git2::Error),
    Domain(String),
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AppError::Io(e) => write!(f, "I/O error: {e}"),
            AppError::Toml(e) => write!(f, "Config error: {e}"),
            AppError::Git(e) => write!(f, "Git error: {e}"),
            AppError::Domain(e) => write!(f, "{e}"),
        }
    }
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        AppError::Io(e)
    }
}

impl From<toml::de::Error> for AppError {
    fn from(e: toml::de::Error) -> Self {
        AppError::Toml(e.to_string())
    }
}

impl From<toml::ser::Error> for AppError {
    fn from(e: toml::ser::Error) -> Self {
        AppError::Toml(e.to_string())
    }
}

impl From<git2::Error> for AppError {
    fn from(e: git2::Error) -> Self {
        AppError::Git(e)
    }
}

pub type Result<T> = std::result::Result<T, AppError>;
