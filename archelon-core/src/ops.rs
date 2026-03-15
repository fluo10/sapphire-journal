//! High-level entry operations shared across CLI, MCP, and future frontends.
//!
//! Each public function accepts already-parsed, typed arguments so that the
//! caller only needs to handle input-format concerns (CLI arg parsing, JSON
//! deserialization, etc.) and output formatting.

use std::{cmp::Ordering, path::{Path, PathBuf}, str::FromStr};

use indexmap::IndexMap;

use caretta_id::CarettaId;
use chrono::{Datelike as _, NaiveDateTime};
use rusqlite::Connection;

use crate::{
    cache,
    entry::{Entry, EntryHeader, EventMeta, Frontmatter, TaskMeta},
    entry_ref::EntryRef,
    error::{Error, Result},
    journal::{DuplicateTitlePolicy, Journal, slugify},
    parser::{read_entry, render_entry},
    period::Period,
};

// ── UpdateOption ──────────────────────────────────────────────────────────────

/// Represents the three possible states for an optional field in an update
/// operation: set it to a new value, clear it (set to `None`), or leave it
/// unchanged.
#[derive(Debug, Default)]
pub enum UpdateOption<T> {
    Set(T),
    Clear,
    #[default]
    Unchanged,
}

// ── SortField / SortOrder ─────────────────────────────────────────────────────

/// Which field to sort entries by in [`list_entries`].
///
/// The default variant is `Unsorted`, which preserves the order returned by the
/// cache or filesystem (no sort applied).  Callers that want to bypass Rust-side
/// sorting — for example to perform locale-aware title sorting in a frontend —
/// should leave `sort_by` as `Unsorted` and sort the results themselves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortField {
    /// No sort — preserve cache / filesystem order.
    #[default]
    Unsorted,
    Id,
    Title,
    TaskStatus,
    CreatedAt,
    UpdatedAt,
    TaskDue,
    EventStart,
}

impl FromStr for SortField {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "id"           => Ok(Self::Id),
            "title"        => Ok(Self::Title),
            "task_status"  => Ok(Self::TaskStatus),
            "created_at"   => Ok(Self::CreatedAt),
            "updated_at"   => Ok(Self::UpdatedAt),
            "task_due"     => Ok(Self::TaskDue),
            "event_start"  => Ok(Self::EventStart),
            other => Err(format!(
                "unknown sort field `{other}`; expected one of: \
                 id, title, task_status, created_at, updated_at, task_due, event_start"
            )),
        }
    }
}

/// Sort direction for [`list_entries`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortOrder {
    #[default]
    Asc,
    Desc,
}

impl FromStr for SortOrder {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "asc"  => Ok(Self::Asc),
            "desc" => Ok(Self::Desc),
            other  => Err(format!("unknown sort order `{other}`; expected `asc` or `desc`")),
        }
    }
}

// ── FieldSelector ─────────────────────────────────────────────────────────────

/// Selects which fields and semantic task conditions to filter on.
///
/// When **all** fields are `false` (the default) and a `period` is set, the period is
/// applied to every timestamp field simultaneously (OR — "all-fields" fallback).  Setting
/// any field to `true` disables that fallback so only the selected conditions apply.
///
/// **Period-field selectors** (`event_span`, `created_at`, `updated_at`): restrict period
/// matching to specific timestamp fields.  Without a `period`, a `true` flag means "the
/// field must be present (set)".
///
/// **Semantic task selectors** — use the period end as a cutoff where applicable:
/// - `task_overdue`: `closed_at` absent **and** `due ≤ period_end` (or `due < now`).
/// - `task_in_progress`: `closed_at` absent **and** `started_at ≤ period_end` (or set).
/// - `task_unstarted`: `started_at` and `closed_at` both absent; period is not applied.
#[derive(Debug, Default, Clone)]
pub struct FieldSelector {
    pub task_overdue: bool,
    pub task_in_progress: bool,
    pub task_unstarted: bool,
    pub event_span: bool,
    pub created_at: bool,
    pub updated_at: bool,
}

impl FieldSelector {
    /// Returns `true` when no field of any kind is selected.
    ///
    /// When this returns `true` and a `period` is set, the period is applied to all
    /// timestamp fields simultaneously (OR — "all-fields" fallback).
    pub fn is_empty(&self) -> bool {
        !self.task_overdue && !self.task_in_progress && !self.task_unstarted
            && !self.event_span && !self.created_at && !self.updated_at
    }

    /// Return a selector with all fields enabled — equivalent to the `--active` CLI flag.
    ///
    /// Combines `task_overdue`, `task_in_progress`, `event_span`, `created_at`, and
    /// `updated_at`.  `task_unstarted` is intentionally excluded because unstarted tasks
    /// carry no timestamp relationship to the period.
    pub fn active() -> Self {
        Self {
            task_overdue:     true,
            task_in_progress: true,
            task_unstarted:   false,
            event_span:       true,
            created_at:       true,
            updated_at:       true,
        }
    }
}

// ── EntryFilter ───────────────────────────────────────────────────────────────

/// Filter criteria for [`list_entries`].
///
/// `period` combined with `fields` forms the timestamp filter:
/// - `period` set, `fields` empty → apply the period to all timestamp fields (OR).
/// - `period` set, `fields` non-empty → apply the period to only the selected fields (OR).
/// - `period` absent, `fields` non-empty → include entries where the selected fields exist.
///
/// `task_status` and `tags` are ANDed on top.
#[derive(Debug, Default)]
pub struct EntryFilter {
    /// Period to match against timestamp fields.
    pub period: Option<Period>,
    /// Which fields the period applies to (empty = all fields).
    pub fields: FieldSelector,
    /// AND condition on task status (empty = no constraint).
    pub task_status: Vec<String>,
    /// AND condition: entry must contain ALL of these tags (empty = no constraint).
    pub tags: Vec<String>,
    /// Field to sort results by (default: `Unsorted` = preserve cache/filesystem order).
    pub sort_by: SortField,
    /// Sort direction (default: ascending).
    pub sort_order: SortOrder,
}

impl EntryFilter {
    pub fn has_timestamp_filter(&self) -> bool {
        self.period.is_some() || !self.fields.is_empty()
    }

    pub fn has_any_filter(&self) -> bool {
        self.has_timestamp_filter() || !self.task_status.is_empty() || !self.tags.is_empty()
    }

    /// Evaluate whether `entry` should be included.
    ///
    /// Returns `(include, labels)` where `labels` lists which timestamp fields
    /// matched (empty when no timestamp filter is active).
    pub fn matches(&self, entry: &EntryHeader) -> (bool, Vec<MatchLabel>) {
        let mut labels = Vec::new();

        let timestamp_ok = if self.has_timestamp_filter() {
            let event_start_val = entry.frontmatter.event.as_ref().map(|e| e.start);
            let event_end_val   = entry.frontmatter.event.as_ref().map(|e| e.end);
            let created_val     = Some(entry.frontmatter.created_at);
            let updated_val     = Some(entry.frontmatter.updated_at);

            if let Some(p) = &self.period {
                // No selectors at all → apply period to all timestamp fields simultaneously (OR).
                // Any explicit selector (including semantic task selectors) disables this fallback.
                let all = self.fields.is_empty();
                if (all || self.fields.event_span) && p.overlaps_event(event_start_val, event_end_val) { labels.push(MatchLabel::EventSpan); }
                if (all || self.fields.created_at) && p.matches(created_val)                           { labels.push(MatchLabel::CreatedAt); }
                if (all || self.fields.updated_at) && p.matches(updated_val)                           { labels.push(MatchLabel::UpdatedAt); }

                // task_overdue: incomplete task with due ≤ period end
                if self.fields.task_overdue {
                    if let Period::Range(_, end) = p {
                        let is_overdue = entry.frontmatter.task.as_ref().is_some_and(|t| {
                            t.closed_at.is_none() && t.due.is_some_and(|due| due <= *end)
                        });
                        if is_overdue { labels.push(MatchLabel::TaskOverdue); }
                    }
                }

                // task_in_progress: incomplete task with started_at ≤ period end
                if self.fields.task_in_progress {
                    if let Period::Range(_, end) = p {
                        let is_in_progress = entry.frontmatter.task.as_ref().is_some_and(|t| {
                            t.closed_at.is_none() && t.started_at.is_some_and(|sa| sa <= *end)
                        });
                        if is_in_progress { labels.push(MatchLabel::TaskInProgress); }
                    }
                }

                // task_unstarted: task that exists but has not been started (period not applied)
                if self.fields.task_unstarted {
                    let is_unstarted = entry.frontmatter.task.as_ref().is_some_and(|t| {
                        t.started_at.is_none() && t.closed_at.is_none()
                    });
                    if is_unstarted { labels.push(MatchLabel::TaskUnstarted); }
                }
            } else {
                // No period: period-field flags → check that the field exists (is set)
                if self.fields.event_span && (event_start_val.is_some() || event_end_val.is_some()) { labels.push(MatchLabel::EventSpan); }
                // created_at / updated_at are always set on every entry → no useful existence check

                // task_overdue: incomplete task with due < now
                if self.fields.task_overdue {
                    let now = chrono::Local::now().naive_local();
                    let is_overdue = entry.frontmatter.task.as_ref().is_some_and(|t| {
                        t.closed_at.is_none() && t.due.is_some_and(|due| due < now)
                    });
                    if is_overdue { labels.push(MatchLabel::TaskOverdue); }
                }

                // task_in_progress: incomplete task with started_at set
                if self.fields.task_in_progress {
                    let is_in_progress = entry.frontmatter.task.as_ref().is_some_and(|t| {
                        t.closed_at.is_none() && t.started_at.is_some()
                    });
                    if is_in_progress { labels.push(MatchLabel::TaskInProgress); }
                }

                // task_unstarted: task that exists but has not been started
                if self.fields.task_unstarted {
                    let is_unstarted = entry.frontmatter.task.as_ref().is_some_and(|t| {
                        t.started_at.is_none() && t.closed_at.is_none()
                    });
                    if is_unstarted { labels.push(MatchLabel::TaskUnstarted); }
                }
            }

            labels.dedup();
            !labels.is_empty()
        } else {
            true
        };

        let status_ok = if !self.task_status.is_empty() {
            entry.frontmatter.task.as_ref().is_some_and(|t| {
                let s = t.status.as_str();
                self.task_status.iter().any(|ts| ts == s)
            })
        } else {
            true
        };

        let tags_ok = if !self.tags.is_empty() {
            self.tags.iter().all(|tag| entry.frontmatter.tags.contains(tag))
        } else {
            true
        };

        (timestamp_ok && status_ok && tags_ok, labels)
    }
}

// ── MatchLabel ────────────────────────────────────────────────────────────────

/// Identifies which timestamp field caused an entry to match a filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchLabel {
    /// Task is incomplete (`closed_at` absent) and `due ≤ period_end` (or `due < now`).
    TaskOverdue,
    /// Task is incomplete (`closed_at` absent) and `started_at ≤ period_end` (or `started_at` set).
    TaskInProgress,
    /// Task exists but `started_at` and `closed_at` are both absent.
    TaskUnstarted,
    /// The filter period overlaps the event's [start, end] span.
    EventSpan,
    CreatedAt,
    UpdatedAt,
}

impl MatchLabel {
    pub fn as_str(self) -> &'static str {
        match self {
            MatchLabel::TaskOverdue    => "TASK_OVERDUE",
            MatchLabel::TaskInProgress => "TASK_IN_PROGRESS",
            MatchLabel::TaskUnstarted  => "TASK_UNSTARTED",
            MatchLabel::EventSpan      => "EVENT_SPAN",
            MatchLabel::CreatedAt      => "CREATED",
            MatchLabel::UpdatedAt      => "UPDATED",
        }
    }
}

// ── tree ──────────────────────────────────────────────────────────────────────

/// A node in an entry hierarchy returned by [`build_entry_tree`].
pub struct EntryTreeNode {
    pub entry: EntryHeader,
    pub labels: Vec<MatchLabel>,
    pub children: Vec<EntryTreeNode>,
}

/// Organise a flat list of `(entry, labels)` pairs into a forest (list of
/// root trees) based on each entry's `parent_id`.
///
/// An entry is treated as a root when its `parent_id` is `None` **or** when
/// its parent is not present in the provided list.  Sibling order within each
/// level mirrors the order of the input slice (i.e. the sort order chosen by
/// the caller is preserved).
pub fn build_entry_tree(entries: Vec<(EntryHeader, Vec<MatchLabel>)>) -> Vec<EntryTreeNode> {
    use std::collections::HashMap;

    // Build an index: CarettaId → position in the input slice.
    let id_index: HashMap<caretta_id::CarettaId, usize> = entries
        .iter()
        .enumerate()
        .map(|(i, (e, _))| (e.frontmatter.id, i))
        .collect();

    // Determine which entries are roots (parent absent or parent not in list).
    let is_root: Vec<bool> = entries
        .iter()
        .map(|(e, _)| {
            e.frontmatter
                .parent_id
                .map_or(true, |pid| !id_index.contains_key(&pid))
        })
        .collect();

    // Build children lists: parent_index → [child_indices] in input order.
    let mut children_of: Vec<Vec<usize>> = vec![Vec::new(); entries.len()];
    for (i, (e, _)) in entries.iter().enumerate() {
        if let Some(pid) = e.frontmatter.parent_id {
            if let Some(&parent_i) = id_index.get(&pid) {
                children_of[parent_i].push(i);
            }
        }
    }

    // Move entries out of the Vec into a parallel structure of Options so we
    // can take ownership during recursive construction without cloning.
    let mut slots: Vec<Option<(EntryHeader, Vec<MatchLabel>)>> =
        entries.into_iter().map(Some).collect();

    fn build_node(
        idx: usize,
        slots: &mut Vec<Option<(EntryHeader, Vec<MatchLabel>)>>,
        children_of: &Vec<Vec<usize>>,
    ) -> EntryTreeNode {
        let (entry, labels) = slots[idx].take().unwrap();
        let children = children_of[idx]
            .iter()
            .map(|&ci| build_node(ci, slots, children_of))
            .collect();
        EntryTreeNode { entry, labels, children }
    }

    (0..slots.len())
        .filter(|&i| is_root[i])
        .map(|i| build_node(i, &mut slots, &children_of))
        .collect()
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
    filter: &EntryFilter,
) -> Result<Vec<(EntryHeader, Vec<MatchLabel>)>> {
    let journal = if let Some(dir) = journal_dir {
        Journal::from_root(dir.to_path_buf())?
    } else {
        Journal::find()?
    };

    // Try the cache first; the sync also keeps it up-to-date as a side effect.
    if let Ok(conn) = cache::open_cache(&journal) {
        let _ = cache::sync_cache(&journal, &conn);
        if let Ok(entries) = cache::list_entries_from_cache(&conn) {
            return apply_filter_and_sort(entries, filter);
        }
    }

    // Fallback: read from disk when the cache is unavailable.
    let paths = journal.collect_entries()?;
    let has_filter = filter.has_any_filter();
    let mut result = Vec::new();
    for p in &paths {
        let header = match read_entry(p) {
            Ok(e) => EntryHeader::from(e),
            Err(e) => {
                eprintln!("warn: {} — {e}", p.display());
                continue;
            }
        };
        let (include, labels) = filter.matches(&header);
        if has_filter && !include {
            continue;
        }
        result.push((header, labels));
    }
    if filter.sort_by != SortField::Unsorted {
        result.sort_by(|(a, _), (b, _)| {
            let ord = sort_cmp(a, b, filter.sort_by);
            if filter.sort_order == SortOrder::Desc { ord.reverse() } else { ord }
        });
    }
    Ok(result)
}

fn apply_filter_and_sort(entries: Vec<EntryHeader>, filter: &EntryFilter) -> Result<Vec<(EntryHeader, Vec<MatchLabel>)>> {
    let has_filter = filter.has_any_filter();
    let mut result = Vec::new();
    for entry in entries {
        let (include, labels) = filter.matches(&entry);
        if has_filter && !include {
            continue;
        }
        result.push((entry, labels));
    }
    if filter.sort_by != SortField::Unsorted {
        result.sort_by(|(a, _), (b, _)| {
            let ord = sort_cmp(a, b, filter.sort_by);
            if filter.sort_order == SortOrder::Desc { ord.reverse() } else { ord }
        });
    }
    Ok(result)
}

fn sort_cmp(a: &EntryHeader, b: &EntryHeader, field: SortField) -> Ordering {
    match field {
        SortField::Unsorted => Ordering::Equal,
        SortField::Id => a.id().cmp(&b.id()),
        SortField::Title => a.title().cmp(b.title()),
        SortField::TaskStatus => {
            let sa = a.frontmatter.task.as_ref().map(|t| t.status.as_str()).unwrap_or("");
            let sb = b.frontmatter.task.as_ref().map(|t| t.status.as_str()).unwrap_or("");
            sa.cmp(sb)
        }
        SortField::CreatedAt  => a.frontmatter.created_at.cmp(&b.frontmatter.created_at),
        SortField::UpdatedAt  => a.frontmatter.updated_at.cmp(&b.frontmatter.updated_at),
        SortField::TaskDue    => cmp_opt(
            a.frontmatter.task.as_ref().and_then(|t| t.due),
            b.frontmatter.task.as_ref().and_then(|t| t.due),
        ),
        SortField::EventStart => cmp_opt(
            a.frontmatter.event.as_ref().map(|e| e.start),
            b.frontmatter.event.as_ref().map(|e| e.start),
        ),
    }
}

/// Compare two `Option<T>` values; `None` sorts after `Some(_)`.
fn cmp_opt<T: Ord>(a: Option<T>, b: Option<T>) -> Ordering {
    match (a, b) {
        (Some(x), Some(y)) => x.cmp(&y),
        (Some(_), None)    => Ordering::Less,
        (None,    Some(_)) => Ordering::Greater,
        (None,    None)    => Ordering::Equal,
    }
}


// ── EntryFields ───────────────────────────────────────────────────────────────

/// Parsed frontmatter fields used for creating or updating an entry.
///
/// All fields are optional.  For [`create_entry`], `title` defaults to an
/// empty string when `None`; `body` defaults to empty.  For [`update_entry`],
/// `None` means "leave unchanged".
#[derive(Debug, Default)]
pub struct EntryFields {
    pub title: Option<String>,
    pub body: Option<String>,
    /// Parent entry reference.  Resolved to a `CarettaId` via the cache at
    /// write time.  `Unchanged` means "leave parent unchanged" in update; "no
    /// parent" in create.  `Clear` sets `parent_id` to `None`.
    pub parent: UpdateOption<EntryRef>,
    pub slug: Option<String>,
    /// `None` = leave tags unchanged; `Some([])` = clear all tags.
    pub tags: Option<Vec<String>>,
    pub task_due: Option<NaiveDateTime>,
    pub task_status: Option<String>,
    pub task_started_at: Option<NaiveDateTime>,
    pub task_closed_at: Option<NaiveDateTime>,
    pub event_start: Option<NaiveDateTime>,
    pub event_end: Option<NaiveDateTime>,
}

// ── create ────────────────────────────────────────────────────────────────────

/// Create a new entry in `journal` with the given `fields`.
///
/// `fields.title` defaults to `""` when `None`; `fields.body` defaults to `""`.
///
/// Fails with:
/// - [`Error::DuplicateTitle`] if another entry already has the same title.
/// - [`Error::DuplicateId`] if the generated ID already exists in the cache
///   (extremely rare in practice).
/// - [`Error::EntryAlreadyExists`] if the destination file already exists on disk.
/// - [`Error::EntryNotFound`] / [`Error::EntryNotFoundByTitle`] if `fields.parent`
///   cannot be resolved.
pub fn create_entry(journal: &Journal, conn: &Connection, fields: EntryFields) -> Result<PathBuf> {
    let id = CarettaId::now_unix();
    let year = chrono::Local::now().year();

    let title = fields.title.unwrap_or_default();
    let body = fields.body.unwrap_or_default();

    // ── duplicate title check ──────────────────────────────────────────────
    if !title.is_empty() {
        let dup_policy = journal.config().unwrap_or_default().journal.duplicate_title;
        if dup_policy != DuplicateTitlePolicy::Allow {
            let count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM entries WHERE title = ?1",
                [&title],
                |row| row.get(0),
            )?;
            if count > 0 {
                match dup_policy {
                    DuplicateTitlePolicy::Warn => {
                        eprintln!("warn: duplicate title detected: `{title}`");
                    }
                    DuplicateTitlePolicy::Error => {
                        return Err(Error::DuplicateTitle(title.clone()));
                    }
                    DuplicateTitlePolicy::Allow => unreachable!(),
                }
            }
        }
    }

    // ── resolve parent ─────────────────────────────────────────────────────
    let parent_id = match &fields.parent {
        UpdateOption::Set(r) => resolve_parent_id(conn, Some(r))?,
        UpdateOption::Clear | UpdateOption::Unchanged => None,
    };

    let tags = fields.tags.unwrap_or_default();

    let task = if fields.task_due.is_some()
        || fields.task_status.is_some()
        || fields.task_started_at.is_some()
        || fields.task_closed_at.is_some()
    {
        let status = fields.task_status.unwrap_or_else(|| "open".to_owned());
        let inactive = matches!(status.as_str(), "done" | "cancelled" | "archived");
        let in_progress = status == "in_progress";
        let started_at = fields
            .task_started_at
            .or_else(|| in_progress.then(|| chrono::Local::now().naive_local()));
        let closed_at = fields
            .task_closed_at
            .or_else(|| inactive.then(|| chrono::Local::now().naive_local()));
        Some(TaskMeta { due: fields.task_due, status, started_at, closed_at, extra: IndexMap::new() })
    } else {
        None
    };

    let event = if fields.event_start.is_some() || fields.event_end.is_some() {
        let start = fields.event_start.or(fields.event_end).unwrap();
        let end = fields.event_end.or(fields.event_start).unwrap();
        Some(EventMeta { start, end, extra: IndexMap::new() })
    } else {
        None
    };

    let now = chrono::Local::now().naive_local();
    let frontmatter = Frontmatter {
        id,
        parent_id,
        title,
        slug: fields.slug.unwrap_or_default(),
        tags,
        created_at: now,
        updated_at: now,
        task,
        event,
        extra: IndexMap::new(),
    };

    let dest = journal.root
        .join(year.to_string())
        .join(entry_filename_from_frontmatter(id, &frontmatter));
    if dest.exists() {
        return Err(Error::EntryAlreadyExists(dest.display().to_string()));
    }

    let entry = Entry { path: dest.clone(), frontmatter, body };

    std::fs::create_dir_all(dest.parent().unwrap())?;
    std::fs::write(&dest, render_entry(&entry))?;
    Ok(dest)
}

// ── update ────────────────────────────────────────────────────────────────────

/// Update the frontmatter of the entry at `path` with non-`None` fields.
///
/// `updated_at` is refreshed automatically by [`write_entry`].
/// If the title or slug changed, the file is also renamed to match the new
/// canonical filename.  Returns `Some(new_path)` when renamed, `None` otherwise.
///
/// Fails with [`Error::DuplicateTitle`] if another entry already uses the new
/// title.  Fails with [`Error::EntryNotFound`] / [`Error::EntryNotFoundByTitle`]
/// if `fields.parent` cannot be resolved.
pub fn update_entry(path: &Path, conn: &Connection, fields: EntryFields) -> Result<Option<PathBuf>> {
    let mut entry = read_entry(path)?;

    if let Some(t) = fields.title {
        // ── duplicate title check (exclude current entry) ──────────────────
        if t != entry.frontmatter.title && !t.is_empty() {
            let count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM entries WHERE title = ?1 AND id != ?2",
                rusqlite::params![t, entry.frontmatter.id],
                |row| row.get(0),
            )?;
            if count > 0 {
                return Err(Error::DuplicateTitle(t));
            }
        }
        entry.frontmatter.title = t;
    }
    if let Some(b) = fields.body {
        entry.body = b;
    }
    match &fields.parent {
        UpdateOption::Set(r) => {
            entry.frontmatter.parent_id = Some(resolve_parent_id(conn, Some(r))?.unwrap());
        }
        UpdateOption::Clear => {
            entry.frontmatter.parent_id = None;
        }
        UpdateOption::Unchanged => {}
    }
    if let Some(s) = fields.slug {
        entry.frontmatter.slug = s;
    }
    if let Some(ts) = fields.tags {
        entry.frontmatter.tags = ts;
    }

    if fields.task_due.is_some()
        || fields.task_status.is_some()
        || fields.task_started_at.is_some()
        || fields.task_closed_at.is_some()
    {
        let task = entry.frontmatter.task.get_or_insert_with(|| TaskMeta {
            status: "open".to_owned(),
            due: None,
            started_at: None,
            closed_at: None,
            extra: IndexMap::new(),
        });
        if let Some(d) = fields.task_due {
            task.due = Some(d);
        }
        if let Some(s) = fields.task_status {
            let in_progress = s == "in_progress";
            let inactive = matches!(s.as_str(), "done" | "cancelled" | "archived");
            task.status = s;
            if in_progress && task.started_at.is_none() && fields.task_started_at.is_none() {
                task.started_at = Some(chrono::Local::now().naive_local());
            }
            if inactive && task.closed_at.is_none() && fields.task_closed_at.is_none() {
                task.closed_at = Some(chrono::Local::now().naive_local());
            }
        }
        if let Some(sa) = fields.task_started_at {
            task.started_at = Some(sa);
        }
        if let Some(ca) = fields.task_closed_at {
            task.closed_at = Some(ca);
        }
    }

    if fields.event_start.is_some() || fields.event_end.is_some() {
        let event = entry.frontmatter.event.get_or_insert_with(|| {
            let start = fields.event_start.or(fields.event_end).unwrap();
            let end = fields.event_end.or(fields.event_start).unwrap();
            EventMeta { start, end, extra: IndexMap::new() }
        });
        if let Some(s) = fields.event_start {
            event.start = s;
        }
        if let Some(e) = fields.event_end {
            event.end = e;
        }
    }

    fix_entry_mut(&mut entry, true)
}

// ── helpers ───────────────────────────────────────────────────────────────────

/// Resolve an optional [`EntryRef`] to the corresponding `CarettaId` by looking
/// up the entry in the cache.  Returns `Ok(None)` when `parent` is `None`.
pub fn resolve_parent_id(conn: &Connection, parent: Option<&EntryRef>) -> Result<Option<CarettaId>> {
    match parent {
        None => Ok(None),
        Some(EntryRef::Id(id)) => Ok(Some(*id)),
        Some(EntryRef::Path(path)) => Ok(Some(read_entry(path)?.frontmatter.id)),
        Some(EntryRef::Title(title)) => {
            Ok(Some(cache::find_entry_by_title(conn, title)?.frontmatter.id))
        }
    }
}

// ── prepare new (for editor workflow) ─────────────────────────────────────────

/// Create a new entry file with a frontmatter template in the journal's year directory.
///
/// The file is named `<id>.md` with required frontmatter pre-filled and optional
/// fields commented out.  The caller should open an editor on the returned path
/// and then call [`fix_entry`] to rename the file once the user has set a title.
///
/// When `parent_id` is `Some`, the `parent_id` field is included in the frontmatter.
pub fn prepare_new_entry(journal: &Journal, parent_id: Option<CarettaId>) -> Result<PathBuf> {
    let id = CarettaId::now_unix();
    let year = chrono::Local::now().year();
    let now = chrono::Local::now().naive_local();
    let now_fmt = now.format("%Y-%m-%dT%H:%M");

    let dir = journal.root.join(year.to_string());
    std::fs::create_dir_all(&dir)?;

    let path = dir.join(format!("{id}.md"));

    let parent_line = match parent_id {
        Some(pid) => format!("parent_id: '{pid}'\n"),
        None => String::new(),
    };

    let template = format!(
        "---\n\
         id: '{id}'\n\
         {parent_line}\
         title: ''\n\
         created_at: {now_fmt}\n\
         updated_at: {now_fmt}\n\
         # slug: ''\n\
         # tags: [tag1, tag2]\n\
         # task:\n\
         #   status: open\n\
         #   due: YYYY-MM-DD\n\
         # event:\n\
         #   start: YYYY-MM-DD\n\
         #   end: YYYY-MM-DD\n\
         ---\n\n"
    );

    std::fs::write(&path, template)?;
    Ok(path)
}

// ── EntryRef resolution ───────────────────────────────────────────────────────

/// Resolve an [`EntryRef`] to a concrete path, opening the journal when needed.
///
/// - `Path` variant: returned as-is.
/// - `Id` variant: looks up by CarettaId (prefix or full) via the cache.
/// - `Title` variant: looks up by exact title (case-sensitive) via the cache.
pub fn resolve_entry(entry_ref: &EntryRef, journal_dir: Option<&Path>) -> Result<PathBuf> {
    match entry_ref {
        EntryRef::Path(p) => Ok(p.clone()),
        EntryRef::Id(id) => {
            let journal = open_journal_for_resolve(journal_dir)?;
            let conn = cache::open_cache(&journal)?;
            cache::sync_cache(&journal, &conn)?;
            cache::find_entry_by_id(&conn, *id).map(|e| e.path)
        }
        EntryRef::Title(title) => {
            let journal = open_journal_for_resolve(journal_dir)?;
            let conn = cache::open_cache(&journal)?;
            cache::sync_cache(&journal, &conn)?;
            cache::find_entry_by_title(&conn, title).map(|e| e.path)
        }
    }
}

fn open_journal_for_resolve(journal_dir: Option<&Path>) -> Result<Journal> {
    if let Some(dir) = journal_dir {
        Journal::from_root(dir.to_path_buf())
    } else {
        Journal::find()
    }
}

// ── CheckIssue ────────────────────────────────────────────────────────────────

/// A problem reported by [`check_entry`].
#[derive(Debug, Clone)]
pub enum CheckIssue {
    /// The filename does not match the ID + title/slug derived from the frontmatter.
    FilenameMismatch {
        /// The filename the entry *should* have.
        expected_filename: String,
    },
}

impl CheckIssue {
    pub fn as_str(&self) -> String {
        match self {
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
    let entry = read_entry(path)?;
    let expected = entry_filename_from_frontmatter(entry.frontmatter.id, &entry.frontmatter);
    let actual = path.file_name().and_then(|s| s.to_str()).unwrap_or_default();

    let mut issues = Vec::new();
    if actual != expected {
        issues.push(CheckIssue::FilenameMismatch { expected_filename: expected });
    }
    Ok(issues)
}

// ── fix ───────────────────────────────────────────────────────────────────────

/// If the entry has a task with `in_progress` status and no `started_at`, set it to now.
fn sync_started_at(entry: &mut Entry) {
    if let Some(task) = &mut entry.frontmatter.task {
        if task.status == "in_progress" && task.started_at.is_none() {
            task.started_at = Some(chrono::Local::now().naive_local());
        }
    }
}

/// If the entry has a task with a closed status and no `closed_at`, set it to now.
fn sync_closed_at(entry: &mut Entry) {
    if let Some(task) = &mut entry.frontmatter.task {
        let is_closed = matches!(task.status.as_str(), "done" | "cancelled" | "archived");
        if is_closed && task.closed_at.is_none() {
            task.closed_at = Some(chrono::Local::now().naive_local());
        }
    }
}

/// Core fix logic on an already-loaded entry: sync `closed_at`, optionally update
/// `updated_at`, write the file, and rename it if the filename no longer matches the
/// frontmatter.
///
/// `touch`: if `true`, refresh `updated_at` to the current time before writing.
///
/// Returns `Some(new_path)` if the file was renamed, `None` otherwise.
fn fix_entry_mut(entry: &mut Entry, touch: bool) -> Result<Option<PathBuf>> {
    sync_started_at(entry);
    sync_closed_at(entry);
    if touch {
        entry.frontmatter.updated_at = chrono::Local::now().naive_local();
    }
    std::fs::write(&entry.path, render_entry(entry))?;

    let expected = entry_filename_from_frontmatter(entry.frontmatter.id, &entry.frontmatter);
    let path = entry.path.clone();
    let actual = path.file_name().and_then(|s| s.to_str()).unwrap_or_default();

    if actual == expected {
        return Ok(None);
    }

    let new_path = path.parent().unwrap_or_else(|| Path::new(".")).join(&expected);
    std::fs::rename(&path, &new_path)?;
    Ok(Some(new_path))
}

/// Normalize an entry: sync `closed_at`, rename the file to match its frontmatter
/// ID and title/slug, and optionally refresh `updated_at`.
///
/// `touch`: if `true`, update `updated_at` to the current time.
///
/// Returns `Some(new_path)` if the file was renamed, `None` if it was already correct.
/// Returns `Err` if the file is not a managed entry.
pub fn fix_entry(path: &Path, touch: bool) -> Result<Option<PathBuf>> {
    let mut entry = read_entry(path)?;
    fix_entry_mut(&mut entry, touch)
}

// ── remove ────────────────────────────────────────────────────────────────────

/// Delete an entry file from disk.
pub fn remove_entry(path: &Path) -> Result<()> {
    std::fs::remove_file(path).map_err(Error::Io)
}

// ── internal helpers ──────────────────────────────────────────────────────────

/// Build the canonical filename for an entry using the frontmatter slug (if set)
/// or `slugify(title)` as a fallback.
pub(crate) fn entry_filename_from_frontmatter(id: CarettaId, fm: &Frontmatter) -> String {
    let slug = if !fm.slug.is_empty() {
        fm.slug.clone()
    } else if fm.title.is_empty() {
        String::new()
    } else {
        slugify(&fm.title)
    };
    if slug.is_empty() {
        format!("{id}.md")
    } else {
        format!("{id}_{slug}.md")
    }
}
