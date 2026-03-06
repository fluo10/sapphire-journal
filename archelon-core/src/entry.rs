use caretta_id::CarettaId;
use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Frontmatter metadata stored at the top of each .md file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Frontmatter {
    pub id: CarettaId,

    #[serde(default)]
    pub title: String,

    /// Optional slug override. If absent, the slug is derived from the filename.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,

    /// Timestamp when the entry was first created. Set automatically by `new`.
    #[serde(default)]
    pub created_at: NaiveDateTime,

    /// Timestamp of the last write. Updated automatically by `write_entry`.
    #[serde(default)]
    pub updated_at: NaiveDateTime,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,

    /// Task metadata. Present only when this entry represents a task.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task: Option<TaskMeta>,

    /// Event metadata. Present only when this entry represents a calendar event.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event: Option<EventMeta>,
}

impl Frontmatter {
    pub fn is_empty(&self) -> bool {
        false
    }
}

/// Task-specific metadata.
///
/// Conventional `status` values: `open`, `in_progress`, `done`, `cancelled`, `archived`.
/// Any custom string is also accepted.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TaskMeta {
    /// Due date/time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due: Option<NaiveDateTime>,

    /// Task status. Conventional values: open | in_progress | done | cancelled | archived
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,

    /// Timestamp when the task was closed (status → done/cancelled/archived).
    /// Set automatically by `entry set`; can be overridden manually.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub closed_at: Option<NaiveDateTime>,
}

/// Event-specific metadata.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EventMeta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start: Option<NaiveDateTime>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub end: Option<NaiveDateTime>,
}

/// A single entry — one Markdown file in the journal.
/// Tasks and notes coexist freely in the body (bullet-journal style).
#[derive(Debug, Clone)]
pub struct Entry {
    /// Absolute path to the source .md file.
    pub path: PathBuf,

    /// Parsed frontmatter. Defaults to empty if the file has none.
    pub frontmatter: Frontmatter,

    /// Raw Markdown body (everything after the frontmatter block).
    pub body: String,
}

impl Entry {
    /// Returns the CarettaId from the frontmatter.
    pub fn id(&self) -> CarettaId {
        self.frontmatter.id
    }

    /// Returns the title: frontmatter title (if non-empty) → file stem → "(untitled)".
    pub fn title(&self) -> &str {
        if !self.frontmatter.title.is_empty() {
            return &self.frontmatter.title;
        }
        self.path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("(untitled)")
    }
}
