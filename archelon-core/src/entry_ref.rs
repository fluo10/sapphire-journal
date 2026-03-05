use std::path::PathBuf;

use crate::{error::Result, journal::Journal};

/// A reference to a journal entry — either a filesystem path or a CarettaId prefix.
///
/// This is the canonical input type for commands that operate on a single entry
/// (show, fix, remove, etc.).  Parse raw user input with [`EntryRef::parse`], then
/// resolve it to a concrete [`PathBuf`] with [`EntryRef::resolve`].
#[derive(Debug, Clone)]
pub enum EntryRef {
    /// A filesystem path to the entry file.
    Path(PathBuf),
    /// A CarettaId prefix (1–7 characters).
    Id(String),
}

impl EntryRef {
    /// Classify a raw string as a path or an ID prefix.
    ///
    /// The string is treated as a **path** when it:
    /// - contains a path separator (`/` or the platform separator), or
    /// - starts with `.` or `~`, or
    /// - ends with `.md`.
    ///
    /// Everything else is treated as a **CarettaId prefix**.
    pub fn parse(s: &str) -> Self {
        if s.contains('/')
            || s.contains(std::path::MAIN_SEPARATOR)
            || s.starts_with('.')
            || s.ends_with(".md")
        {
            EntryRef::Path(PathBuf::from(s))
        } else {
            EntryRef::Id(s.to_owned())
        }
    }

    /// Resolve this reference to a concrete file path.
    ///
    /// - `Path` variant: returns the stored path as-is.
    /// - `Id` variant: delegates to [`Journal::find_entry_by_id`].
    pub fn resolve(&self, journal: &Journal) -> Result<PathBuf> {
        match self {
            EntryRef::Path(p) => Ok(p.clone()),
            EntryRef::Id(id) => journal.find_entry_by_id(id),
        }
    }
}

impl From<&str> for EntryRef {
    fn from(s: &str) -> Self {
        Self::parse(s)
    }
}

impl From<String> for EntryRef {
    fn from(s: String) -> Self {
        Self::parse(&s)
    }
}
