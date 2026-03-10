use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Failed to parse frontmatter: {0}")]
    FrontmatterParse(#[from] serde_yaml::Error),

    #[error("Invalid entry file: {0}")]
    InvalidEntry(String),

    #[error("no .archelon directory found in current directory or any parent")]
    JournalNotFound,

    #[error("no entry found with id prefix `{0}`")]
    EntryNotFound(String),

    #[error("id prefix `{0}` is ambiguous ({1} matches)")]
    AmbiguousId(String, usize),

    #[error("{0} already exists")]
    EntryAlreadyExists(String),

    #[error("invalid config: {0}")]
    InvalidConfig(String),

    #[error("cache error: {0}")]
    Cache(#[from] rusqlite::Error),

    /// The cache DB was created by a newer version of archelon.
    /// The user must either update archelon or recreate the cache.
    #[error(
        "cache schema v{db_version} is newer than this app supports (v{app_version}); \
         update archelon, or recreate the cache with `archelon cache rebuild`"
    )]
    CacheSchemaTooNew { db_version: i32, app_version: i32 },
}

pub type Result<T> = std::result::Result<T, Error>;
