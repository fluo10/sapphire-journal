use caretta_id::CarettaId;
use chrono::NaiveDateTime;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Frontmatter metadata stored at the top of each .md file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Frontmatter {
    pub id: CarettaId,

    /// Parent entry ID for hierarchical (bullet-journal nested) relationships.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<CarettaId>,

    #[serde(default)]
    pub title: String,

    /// Optional slug override. If absent, the slug is derived from the filename.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,

    /// Timestamp when the entry was first created. Set automatically by `new`.
    #[serde(default, with = "naive_datetime_serde")]
    pub created_at: NaiveDateTime,

    /// Timestamp of the last write. Updated automatically by `write_entry`.
    #[serde(default, with = "naive_datetime_serde")]
    pub updated_at: NaiveDateTime,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,

    /// Task metadata. Present only when this entry represents a task.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task: Option<TaskMeta>,

    /// Event metadata. Present only when this entry represents a calendar event.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event: Option<EventMeta>,

    /// Unknown frontmatter fields preserved for round-trip compatibility.
    #[serde(flatten)]
    pub extra: IndexMap<String, serde_yaml::Value>,
}

/// Task-specific metadata.
///
/// Conventional `status` values: `open`, `in_progress`, `done`, `cancelled`, `archived`.
/// Any custom string is also accepted.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskMeta {
    /// Due date/time.
    #[serde(default, skip_serializing_if = "Option::is_none", with = "naive_datetime_serde::eod::opt")]
    pub due: Option<NaiveDateTime>,

    /// Task status. Conventional values: open | in_progress | done | cancelled | archived
    #[serde(default = "default_task_status")]
    pub status: String,

    /// Timestamp when the task was started (status → in_progress).
    /// Set automatically by `entry set`; can be overridden manually.
    #[serde(default, skip_serializing_if = "Option::is_none", with = "naive_datetime_serde::opt")]
    pub started_at: Option<NaiveDateTime>,

    /// Timestamp when the task was closed (status → done/cancelled/archived).
    /// Set automatically by `entry set`; can be overridden manually.
    #[serde(default, skip_serializing_if = "Option::is_none", with = "naive_datetime_serde::opt")]
    pub closed_at: Option<NaiveDateTime>,

    /// Unknown task fields preserved for round-trip compatibility.
    #[serde(flatten)]
    pub extra: IndexMap<String, serde_yaml::Value>,
}

fn default_task_status() -> String {
    "open".to_owned()
}

/// Event-specific metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventMeta {
    #[serde(with = "naive_datetime_serde")]
    pub start: NaiveDateTime,
    #[serde(with = "naive_datetime_serde::eod")]
    pub end: NaiveDateTime,

    /// Unknown event fields preserved for round-trip compatibility.
    #[serde(flatten)]
    pub extra: IndexMap<String, serde_yaml::Value>,
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
        return &self.frontmatter.title;
    }
}

/// Custom serde module for `NaiveDateTime` using minute-precision format (`YYYY-MM-DDTHH:MM`).
///
/// Serializes to `%Y-%m-%dT%H:%M`. Deserializes from:
/// - `%Y-%m-%dT%H:%M`        (minute precision — preferred)
/// - `%Y-%m-%dT%H:%M:%S`     (second precision)
/// - `%Y-%m-%dT%H:%M:%S%.f`  (sub-second precision — for backward compat)
/// - `%Y-%m-%d`              (date only — `00:00` for start fields, `23:59` via `eod` sub-module)
mod naive_datetime_serde {
    use chrono::{NaiveDate, NaiveDateTime};
    use serde::{Deserialize, Deserializer, Serializer};

    const FORMAT: &str = "%Y-%m-%dT%H:%M";

    /// Parse a datetime string, using `(h, m)` as the fallback time when only a date is given.
    pub(super) fn parse_with_fallback(s: &str, h: u32, m: u32) -> Result<NaiveDateTime, String> {
        for fmt in [FORMAT, "%Y-%m-%dT%H:%M:%S", "%Y-%m-%dT%H:%M:%S%.f"] {
            if let Ok(dt) = NaiveDateTime::parse_from_str(s, fmt) {
                return Ok(dt);
            }
        }
        if let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
            return Ok(d.and_hms_opt(h, m, 0).unwrap());
        }
        Err(format!(
            "cannot parse `{s}` as a datetime; expected format YYYY-MM-DDTHH:MM"
        ))
    }

    pub fn serialize<S: Serializer>(dt: &NaiveDateTime, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&dt.format(FORMAT).to_string())
    }

    /// Deserializes with date-only → `00:00` (start-of-day).
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<NaiveDateTime, D::Error> {
        let raw = String::deserialize(d)?;
        parse_with_fallback(&raw, 0, 0).map_err(serde::de::Error::custom)
    }

    /// For `Option<NaiveDateTime>` fields — date-only → `00:00`.
    pub mod opt {
        use chrono::NaiveDateTime;
        use serde::{Deserialize, Deserializer, Serializer};

        pub fn serialize<S: Serializer>(
            opt: &Option<NaiveDateTime>,
            s: S,
        ) -> Result<S::Ok, S::Error> {
            match opt {
                Some(dt) => super::serialize(dt, s),
                None => s.serialize_none(),
            }
        }

        pub fn deserialize<'de, D: Deserializer<'de>>(
            d: D,
        ) -> Result<Option<NaiveDateTime>, D::Error> {
            let raw = String::deserialize(d)?;
            super::parse_with_fallback(&raw, 0, 0).map(Some).map_err(serde::de::Error::custom)
        }
    }

    /// Variant where date-only → `23:59` (end-of-day). Used for due dates and event end times.
    pub mod eod {
        use chrono::NaiveDateTime;
        use serde::{Deserialize, Deserializer, Serializer};

        pub fn serialize<S: Serializer>(dt: &NaiveDateTime, s: S) -> Result<S::Ok, S::Error> {
            super::serialize(dt, s)
        }

        pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<NaiveDateTime, D::Error> {
            let raw = String::deserialize(d)?;
            super::parse_with_fallback(&raw, 23, 59).map_err(serde::de::Error::custom)
        }

        pub mod opt {
            use chrono::NaiveDateTime;
            use serde::{Deserialize, Deserializer, Serializer};

            pub fn serialize<S: Serializer>(
                opt: &Option<NaiveDateTime>,
                s: S,
            ) -> Result<S::Ok, S::Error> {
                match opt {
                    Some(dt) => super::serialize(dt, s),
                    None => s.serialize_none(),
                }
            }

            pub fn deserialize<'de, D: Deserializer<'de>>(
                d: D,
            ) -> Result<Option<NaiveDateTime>, D::Error> {
                let raw = String::deserialize(d)?;
                super::super::parse_with_fallback(&raw, 23, 59)
                    .map(Some)
                    .map_err(serde::de::Error::custom)
            }
        }
    }
}
