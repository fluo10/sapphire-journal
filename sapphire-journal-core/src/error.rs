use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Failed to parse frontmatter: {0}")]
    FrontmatterParse(#[from] serde_yaml::Error),

    #[error("Invalid entry file: {0}")]
    InvalidEntry(String),

    #[error("no .sapphire-journal directory found in current directory or any parent")]
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

    /// The cache DB was created by a newer version of sapphire-journal.
    /// The user must either update sapphire-journal or recreate the cache.
    #[error(
        "cache schema v{db_version} is newer than this app supports (v{app_version}); \
         update sapphire-journal, or recreate the cache with `sapphire-journal cache rebuild`"
    )]
    CacheSchemaTooNew { db_version: i32, app_version: i32 },
}

pub type Result<T> = std::result::Result<T, Error>;

impl From<sapphire_retrieve::Error> for Error {
    fn from(e: sapphire_retrieve::Error) -> Self {
        match e {
            sapphire_retrieve::Error::Sqlite(e) => Error::Cache(e),
            sapphire_retrieve::Error::Embed(s) => Error::Embed(s),
            sapphire_retrieve::Error::Io(e) => Error::Io(e),
            sapphire_retrieve::Error::SchemaTooNew { db_version, app_version } => {
                Error::InvalidConfig(format!(
                    "retrieve DB schema v{db_version} is newer than supported (v{app_version}); \
                     delete the retrieve DB and re-sync"
                ))
            }
        }
    }
}
