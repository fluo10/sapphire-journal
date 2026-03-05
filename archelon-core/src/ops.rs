//! High-level entry operations shared across CLI, MCP, and future frontends.
//!
//! Each public function accepts already-parsed, typed arguments so that the
//! caller only needs to handle input-format concerns (CLI arg parsing, JSON
//! deserialization, etc.) and output formatting.

use std::path::{Path, PathBuf};

use caretta_id::CarettaId;
use chrono::NaiveDateTime;

use crate::{
    entry::{Entry, EventMeta, Frontmatter, TaskMeta},
    entry_ref::EntryRef,
    error::{Error, Result},
    journal::{is_managed_filename, Journal, new_entry_path, slugify},
    parser::{read_entry, render_entry, write_entry},
    period::Period,
};

// ── EntryFilter ───────────────────────────────────────────────────────────────

/// Filter criteria for [`list_entries`].
///
/// Timestamp fields are ORed: an entry is included when *any* specified
/// timestamp filter matches its corresponding field.
///
/// `task_status` is ANDed on top: if non-empty, the entry must also have a
/// task whose status appears in the list.
#[derive(Debug, Default)]
pub struct EntryFilter {
    /// Shortcut: apply the same period to all timestamp fields simultaneously.
    pub period: Option<Period>,
    pub task_due: Option<Period>,
    pub event_start: Option<Period>,
    pub event_end: Option<Period>,
    pub created_at: Option<Period>,
    pub updated_at: Option<Period>,
    /// AND condition on task status (empty = no constraint).
    pub task_status: Vec<String>,
}

impl EntryFilter {
    pub fn has_timestamp_filter(&self) -> bool {
        self.period.is_some()
            || self.task_due.is_some()
            || self.event_start.is_some()
            || self.event_end.is_some()
            || self.created_at.is_some()
            || self.updated_at.is_some()
    }

    pub fn has_any_filter(&self) -> bool {
        self.has_timestamp_filter() || !self.task_status.is_empty()
    }

    /// Evaluate whether `entry` should be included.
    ///
    /// Returns `(include, labels)` where `labels` lists which timestamp fields
    /// matched (empty when no timestamp filter is active).
    pub fn matches(&self, entry: &Entry) -> (bool, Vec<MatchLabel>) {
        let mut labels = Vec::new();

        let timestamp_ok = if self.has_timestamp_filter() {
            let task_due_val = entry.frontmatter.task.as_ref().and_then(|t| t.due);
            let event_start_val = entry.frontmatter.event.as_ref().and_then(|e| e.start);
            let event_end_val = entry.frontmatter.event.as_ref().and_then(|e| e.end);
            let created_val = entry.frontmatter.created_at;
            let updated_val = entry.frontmatter.updated_at;

            macro_rules! check {
                ($filter:expr, $val:expr, $label:expr) => {
                    if let Some(p) = &$filter {
                        if p.matches($val) {
                            labels.push($label);
                        }
                    }
                };
            }

            // --period applies to all fields simultaneously
            if let Some(p) = &self.period {
                if p.matches(task_due_val) { labels.push(MatchLabel::TaskDue); }
                if p.matches(event_start_val) { labels.push(MatchLabel::EventStart); }
                if p.matches(event_end_val) { labels.push(MatchLabel::EventEnd); }
                if p.matches(created_val) { labels.push(MatchLabel::CreatedAt); }
                if p.matches(updated_val) { labels.push(MatchLabel::UpdatedAt); }
            }

            // per-field filters (dedup handles overlap with --period)
            check!(self.task_due,    task_due_val,    MatchLabel::TaskDue);
            check!(self.event_start, event_start_val, MatchLabel::EventStart);
            check!(self.event_end,   event_end_val,   MatchLabel::EventEnd);
            check!(self.created_at,  created_val,     MatchLabel::CreatedAt);
            check!(self.updated_at,  updated_val,     MatchLabel::UpdatedAt);

            labels.dedup();
            !labels.is_empty()
        } else {
            true
        };

        let status_ok = if !self.task_status.is_empty() {
            entry.frontmatter.task.as_ref().is_some_and(|t| {
                let s = t.status.as_deref().unwrap_or("open");
                self.task_status.iter().any(|ts| ts == s)
            })
        } else {
            true
        };

        (timestamp_ok && status_ok, labels)
    }
}

// ── MatchLabel ────────────────────────────────────────────────────────────────

/// Identifies which timestamp field caused an entry to match a filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchLabel {
    TaskDue,
    EventStart,
    EventEnd,
    CreatedAt,
    UpdatedAt,
}

impl MatchLabel {
    pub fn as_str(self) -> &'static str {
        match self {
            MatchLabel::TaskDue    => "TASK_DUE",
            MatchLabel::EventStart => "EVENT_START",
            MatchLabel::EventEnd   => "EVENT_END",
            MatchLabel::CreatedAt  => "CREATED",
            MatchLabel::UpdatedAt  => "UPDATED",
        }
    }
}

// ── list ──────────────────────────────────────────────────────────────────────

/// Collect and filter journal entries.
///
/// - `journal_dir`: explicit journal root override (`None` = auto-detect)
/// - `path`: scan only this directory instead of the journal (`None` = use journal)
/// - `filter`: filter criteria; all fields are optional
///
/// Returns `(entry, match_labels)` pairs.  When no filter is active every
/// entry is returned with an empty label list.
pub fn list_entries(
    journal_dir: Option<&Path>,
    path: Option<&Path>,
    filter: &EntryFilter,
) -> Result<Vec<(Entry, Vec<MatchLabel>)>> {
    let paths = collect_entries(journal_dir, path)?;
    let has_filter = filter.has_any_filter();
    let mut result = Vec::new();

    for p in &paths {
        if !is_managed_filename(p) {
            continue;
        }
        let entry = match read_entry(p) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("warn: {} — {e}", p.display());
                continue;
            }
        };
        let (include, labels) = filter.matches(&entry);
        if has_filter && !include {
            continue;
        }
        result.push((entry, labels));
    }

    Ok(result)
}

/// Collect `.md` file paths for listing.
///
/// Priority:
/// 1. `path` argument — scan only that directory
/// 2. `journal_dir` argument — use journal root + year subdirs
/// 3. Auto-detect journal from CWD
/// 4. Fall back to `"."`
pub fn collect_entries(journal_dir: Option<&Path>, path: Option<&Path>) -> Result<Vec<PathBuf>> {
    if let Some(v) = path {
        let mut paths: Vec<_> = std::fs::read_dir(v)?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("md"))
            .collect();
        paths.sort();
        return Ok(paths);
    }

    if let Some(dir) = journal_dir {
        return Journal::from_root(dir.to_path_buf())?.collect_entries();
    }

    if let Ok(journal) = Journal::find() {
        return journal.collect_entries();
    }

    let mut paths: Vec<_> = std::fs::read_dir(".")?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("md"))
        .collect();
    paths.sort();
    Ok(paths)
}

// ── EntryFields ───────────────────────────────────────────────────────────────

/// Parsed frontmatter fields used for creating or updating an entry.
///
/// All fields are optional. Callers (CLI, MCP, …) parse their input format
/// into this type before calling [`create_entry`] or [`update_entry`].
#[derive(Debug, Default)]
pub struct EntryFields {
    pub title: Option<String>,
    pub slug: Option<String>,
    /// `None` = leave tags unchanged; `Some([])` = clear all tags.
    pub tags: Option<Vec<String>>,
    pub task_due: Option<NaiveDateTime>,
    pub task_status: Option<String>,
    pub task_closed_at: Option<NaiveDateTime>,
    pub event_start: Option<NaiveDateTime>,
    pub event_end: Option<NaiveDateTime>,
}

// ── create ────────────────────────────────────────────────────────────────────

/// Create a new entry in `journal` with the given `name`, `body`, and `fields`.
///
/// Returns the path of the newly created file.
/// Fails with [`Error::EntryAlreadyExists`] if the destination already exists.
pub fn create_entry(
    journal: &Journal,
    name: &str,
    body: String,
    fields: EntryFields,
) -> Result<PathBuf> {
    let dest = journal.root.join(new_entry_path(name));
    if dest.exists() {
        return Err(Error::EntryAlreadyExists(dest.display().to_string()));
    }

    let fm_title = fields.title.or_else(|| Some(name.to_owned()));
    let tags = fields.tags.unwrap_or_default();

    let task = if fields.task_due.is_some()
        || fields.task_status.is_some()
        || fields.task_closed_at.is_some()
    {
        let inactive =
            matches!(fields.task_status.as_deref(), Some("done" | "cancelled" | "archived"));
        let closed_at = fields
            .task_closed_at
            .or_else(|| inactive.then(|| chrono::Local::now().naive_local()));
        Some(TaskMeta { due: fields.task_due, status: fields.task_status, closed_at })
    } else {
        None
    };

    let event = if fields.event_start.is_some() || fields.event_end.is_some() {
        Some(EventMeta { start: fields.event_start, end: fields.event_end })
    } else {
        None
    };

    let now = chrono::Local::now().naive_local();
    let entry = Entry {
        path: dest.clone(),
        frontmatter: Frontmatter {
            title: fm_title,
            slug: fields.slug,
            tags,
            created_at: Some(now),
            updated_at: Some(now),
            task,
            event,
        },
        body,
    };

    std::fs::create_dir_all(dest.parent().unwrap())?;
    std::fs::write(&dest, render_entry(&entry))?;
    Ok(dest)
}

// ── update ────────────────────────────────────────────────────────────────────

/// Update the frontmatter of the entry at `path` with non-`None` fields.
///
/// `updated_at` is refreshed automatically by [`write_entry`].
pub fn update_entry(path: &Path, fields: EntryFields) -> Result<()> {
    let mut entry = read_entry(path)?;

    if let Some(t) = fields.title {
        entry.frontmatter.title = Some(t);
    }
    if let Some(s) = fields.slug {
        entry.frontmatter.slug = Some(s);
    }
    if let Some(ts) = fields.tags {
        entry.frontmatter.tags = ts;
    }

    if fields.task_due.is_some()
        || fields.task_status.is_some()
        || fields.task_closed_at.is_some()
    {
        let task = entry.frontmatter.task.get_or_insert_with(Default::default);
        if let Some(d) = fields.task_due {
            task.due = Some(d);
        }
        if let Some(s) = fields.task_status {
            let inactive = matches!(s.as_str(), "done" | "cancelled" | "archived");
            task.status = Some(s);
            if inactive && task.closed_at.is_none() && fields.task_closed_at.is_none() {
                task.closed_at = Some(chrono::Local::now().naive_local());
            }
        }
        if let Some(ca) = fields.task_closed_at {
            task.closed_at = Some(ca);
        }
    }

    if fields.event_start.is_some() || fields.event_end.is_some() {
        let event = entry.frontmatter.event.get_or_insert_with(Default::default);
        if let Some(s) = fields.event_start {
            event.start = Some(s);
        }
        if let Some(e) = fields.event_end {
            event.end = Some(e);
        }
    }

    write_entry(&mut entry)?;
    Ok(())
}

// ── EntryRef resolution ───────────────────────────────────────────────────────

/// Resolve an [`EntryRef`] to a concrete path, opening the journal when needed.
///
/// - `Path` variant: returned as-is.
/// - `Id` variant: searches the journal found via `journal_dir` (or auto-detected).
pub fn resolve_entry(entry_ref: &EntryRef, journal_dir: Option<&Path>) -> Result<PathBuf> {
    match entry_ref {
        EntryRef::Path(p) => Ok(p.clone()),
        EntryRef::Id(id) => {
            let journal = if let Some(dir) = journal_dir {
                Journal::from_root(dir.to_path_buf())?
            } else {
                Journal::find()?
            };
            journal.find_entry_by_id(id)
        }
    }
}

// ── CheckIssue ────────────────────────────────────────────────────────────────

/// A problem reported by [`check_entry`].
#[derive(Debug, Clone)]
pub enum CheckIssue {
    /// The file does not follow the archelon-managed filename convention.
    UnmanagedFilename,
    /// The filename does not match the ID + title/slug derived from the frontmatter.
    FilenameMismatch {
        /// The filename the entry *should* have.
        expected_filename: String,
    },
}

impl CheckIssue {
    pub fn as_str(&self) -> String {
        match self {
            CheckIssue::UnmanagedFilename =>
                "not a managed entry (filename lacks a valid CarettaId prefix)".to_owned(),
            CheckIssue::FilenameMismatch { expected_filename } =>
                format!("filename mismatch — should be `{expected_filename}`"),
        }
    }
}

// ── check ─────────────────────────────────────────────────────────────────────

/// Validate an entry's frontmatter and filename.
///
/// Returns a (possibly empty) list of [`CheckIssue`]s.
/// An empty list means the entry passes all checks.
pub fn check_entry(path: &Path) -> Result<Vec<CheckIssue>> {
    if !is_managed_filename(path) {
        return Ok(vec![CheckIssue::UnmanagedFilename]);
    }

    let entry = read_entry(path)?;
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
    // Safety: `is_managed_filename` guarantees a valid 7-char CarettaId prefix.
    let id: CarettaId = stem[..7].parse().unwrap();
    let expected = entry_filename_from_frontmatter(id, &entry.frontmatter);
    let actual = path.file_name().and_then(|s| s.to_str()).unwrap_or_default();

    let mut issues = Vec::new();
    if actual != expected {
        issues.push(CheckIssue::FilenameMismatch { expected_filename: expected });
    }
    Ok(issues)
}

// ── fix ───────────────────────────────────────────────────────────────────────

/// Rename an entry file so its name matches the frontmatter ID and title/slug.
///
/// Returns `Some(new_path)` if the file was renamed, `None` if it was already correct.
/// Returns `Err` if the file is not a managed entry.
pub fn fix_entry(path: &Path) -> Result<Option<PathBuf>> {
    if !is_managed_filename(path) {
        return Err(Error::InvalidEntry(format!(
            "{}: not a managed entry (filename lacks a valid CarettaId prefix)",
            path.display()
        )));
    }

    let entry = read_entry(path)?;
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
    let id: CarettaId = stem[..7].parse().unwrap();

    let expected = entry_filename_from_frontmatter(id, &entry.frontmatter);
    let actual = path.file_name().and_then(|s| s.to_str()).unwrap_or_default();

    if actual == expected {
        return Ok(None);
    }

    let new_path = path.parent().unwrap_or_else(|| Path::new(".")).join(&expected);
    std::fs::rename(path, &new_path)?;
    Ok(Some(new_path))
}

// ── remove ────────────────────────────────────────────────────────────────────

/// Delete an entry file from disk.
pub fn remove_entry(path: &Path) -> Result<()> {
    std::fs::remove_file(path).map_err(Error::Io)
}

// ── internal helpers ──────────────────────────────────────────────────────────

/// Build the canonical filename for an entry using the frontmatter slug (if set)
/// or `slugify(title)` as a fallback.
fn entry_filename_from_frontmatter(id: CarettaId, fm: &Frontmatter) -> String {
    let slug = fm.slug.clone().unwrap_or_else(|| {
        fm.title.as_deref().map(slugify).unwrap_or_default()
    });
    if slug.is_empty() {
        format!("{id}.md")
    } else {
        format!("{id}_{slug}.md")
    }
}
