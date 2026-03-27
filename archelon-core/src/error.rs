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

    #[error("no entry found with title `{0}`")]
    EntryNotFoundByTitle(String),

    #[error("title `{0}` is ambiguous ({1} matches)")]
    AmbiguousTitle(String, usize),

    #[error("duplicate title `{0}` — set duplicate_title = \"allow\" in config to permit this")]
    DuplicateTitle(String),

    #[error("duplicate id `{0}` found in files: {1} and {2}")]
    DuplicateId(String, String, String),

    #[error("{0} already exists")]
    EntryAlreadyExists(String),

    #[error("invalid config: {0}")]
    InvalidConfig(String),

    #[error("cache error: {0}")]
    Cache(#[from] rusqlite::Error),

    #[error("embedding error: {0}")]
    Embed(String),

    /// The cache DB was created by a newer version of archelon.
    /// The user must either update archelon or recreate the cache.
    #[error(
        "cache schema v{db_version} is newer than this app supports (v{app_version}); \
         update archelon, or recreate the cache with `archelon cache rebuild`"
    )]
    CacheSchemaTooNew { db_version: i32, app_version: i32 },
}

pub type Result<T> = std::result::Result<T, Error>;

impl From<archelon_retrieve::Error> for Error {
    fn from(e: archelon_retrieve::Error) -> Self {
        match e {
            archelon_retrieve::Error::Sqlite(e) => Error::Cache(e),
            archelon_retrieve::Error::Embed(s) => Error::Embed(s),
            archelon_retrieve::Error::Io(e) => Error::Io(e),
            archelon_retrieve::Error::SchemaTooNew { db_version, app_version } => {
                Error::InvalidConfig(format!(
                    "retrieve DB schema v{db_version} is newer than supported (v{app_version}); \
                     delete the retrieve DB and re-sync"
                ))
            }
        }
    }
}
