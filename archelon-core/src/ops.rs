//! High-level entry operations shared across CLI, MCP, and future frontends.
//!
//! Each public function accepts already-parsed, typed arguments so that the
//! caller only needs to handle input-format concerns (CLI arg parsing, JSON
//! deserialization, etc.) and output formatting.

use std::{cmp::Ordering, path::{Path, PathBuf}, str::FromStr};

use indexmap::IndexMap;

use caretta_id::CarettaId;
use chrono::{Datelike as _, NaiveDateTime};

use crate::{
    cache,
    entry::{Entry, EventMeta, Frontmatter, TaskMeta},
    entry_ref::EntryRef,
    error::{Error, Result},
    journal::{Journal, slugify},
    parser::{read_entry, render_entry},
    period::Period,
};

// ── SortField / SortOrder ─────────────────────────────────────────────────────

/// Which field to sort entries by in [`list_entries`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortField {
    #[default]
    Id,
    Title,
    TaskStatus,
    CreatedAt,
    UpdatedAt,
    TaskDue,
    EventStart,
    EventEnd,
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
            "event_end"    => Ok(Self::EventEnd),
            other => Err(format!(
                "unknown sort field `{other}`; expected one of: \
                 id, title, task_status, created_at, updated_at, task_due, event_start, event_end"
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

/// Selects which timestamp fields [`EntryFilter::period`] applies to.
///
/// When all fields are `false` (the default), `period` applies to all fields
/// simultaneously (OR across all).  Setting one or more fields to `true`
/// restricts the period check to only those fields.
///
/// Without a `period`, a `true` flag means "the field must be present (set)".
#[derive(Debug, Default, Clone)]
pub struct FieldSelector {
    pub task_due: bool,
    pub event_span: bool,
    pub created_at: bool,
    pub updated_at: bool,
}

impl FieldSelector {
    /// Returns `true` when no field is explicitly selected (i.e. all are `false`).
    pub fn is_empty(&self) -> bool {
        !self.task_due && !self.event_span && !self.created_at && !self.updated_at
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
    /// OR condition with timestamp filters: include tasks whose `due` is in the past
    /// and `closed_at` is absent.
    pub overdue: bool,
    /// OR condition with timestamp filters: include tasks that have `started_at` set,
    /// `closed_at` absent, and (when a period is active) `started_at` ≤ period end.
    pub task_started: bool,
    /// Field to sort results by (`None` = keep filesystem order).
    pub sort_by: Option<SortField>,
    /// Sort direction (default: ascending).
    pub sort_order: SortOrder,
}

impl EntryFilter {
    pub fn has_timestamp_filter(&self) -> bool {
        self.period.is_some() || !self.fields.is_empty() || self.overdue || self.task_started
    }

    pub fn has_any_filter(&self) -> bool {
        self.has_timestamp_filter() || !self.task_status.is_empty() || !self.tags.is_empty()
    }

    /// Evaluate whether `entry` should be included.
    ///
    /// Returns `(include, labels)` where `labels` lists which timestamp fields
    /// matched (empty when no timestamp filter is active).
    pub fn matches(&self, entry: &Entry) -> (bool, Vec<MatchLabel>) {
        let mut labels = Vec::new();

        let timestamp_ok = if self.has_timestamp_filter() {
            let task_due_val    = entry.frontmatter.task.as_ref().and_then(|t| t.due);
            let event_start_val = entry.frontmatter.event.as_ref().map(|e| e.start);
            let event_end_val   = entry.frontmatter.event.as_ref().map(|e| e.end);
            let created_val     = Some(entry.frontmatter.created_at);
            let updated_val     = Some(entry.frontmatter.updated_at);

            if let Some(p) = &self.period {
                // No field selectors → apply to all fields simultaneously
                let all = self.fields.is_empty();
                if (all || self.fields.task_due)   && p.matches(task_due_val)                        { labels.push(MatchLabel::TaskDue); }
                if (all || self.fields.event_span) && p.overlaps_event(event_start_val, event_end_val) { labels.push(MatchLabel::EventSpan); }
                if (all || self.fields.created_at) && p.matches(created_val)                         { labels.push(MatchLabel::CreatedAt); }
                if (all || self.fields.updated_at) && p.matches(updated_val)                         { labels.push(MatchLabel::UpdatedAt); }
            } else {
                // No period: field flags → check that the field exists (is set)
                if self.fields.task_due   && task_due_val.is_some()                                   { labels.push(MatchLabel::TaskDue); }
                if self.fields.event_span && (event_start_val.is_some() || event_end_val.is_some())   { labels.push(MatchLabel::EventSpan); }
                // created_at / updated_at are always set on every entry → no useful existence check
            }

            // overdue: task with due in the past and no closed_at
            if self.overdue {
                let is_overdue = entry.frontmatter.task.as_ref().is_some_and(|t| {
                    t.due.is_some_and(|due| due < chrono::Local::now().naive_local())
                        && t.closed_at.is_none()
                });
                if is_overdue {
                    labels.push(MatchLabel::Overdue);
                }
            }

            // task_started: task with started_at set, no closed_at, and started_at ≤ period end
            if self.task_started {
                let period_end = match &self.period {
                    Some(Period::Range(_, end)) => Some(*end),
                    _ => None,
                };
                let is_started = entry.frontmatter.task.as_ref().is_some_and(|t| {
                    t.started_at.is_some()
                        && t.closed_at.is_none()
                        && period_end.map_or(true, |end| t.started_at.unwrap() <= end)
                });
                if is_started {
                    labels.push(MatchLabel::TaskStarted);
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
    TaskDue,
    TaskStarted,
    /// The filter period overlaps the event's [start, end] span.
    EventSpan,
    CreatedAt,
    UpdatedAt,
    Overdue,
}

impl MatchLabel {
    pub fn as_str(self) -> &'static str {
        match self {
            MatchLabel::TaskDue     => "TASK_DUE",
            MatchLabel::TaskStarted => "TASK_STARTED",
            MatchLabel::EventSpan   => "EVENT_SPAN",
            MatchLabel::CreatedAt   => "CREATED",
            MatchLabel::UpdatedAt   => "UPDATED",
            MatchLabel::Overdue     => "OVERDUE",
        }
    }
}

// ── tree ──────────────────────────────────────────────────────────────────────

/// A node in an entry hierarchy returned by [`build_entry_tree`].
pub struct EntryTreeNode {
    pub entry: Entry,
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
pub fn build_entry_tree(entries: Vec<(Entry, Vec<MatchLabel>)>) -> Vec<EntryTreeNode> {
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
    let mut slots: Vec<Option<(Entry, Vec<MatchLabel>)>> =
        entries.into_iter().map(Some).collect();

    fn build_node(
        idx: usize,
        slots: &mut Vec<Option<(Entry, Vec<MatchLabel>)>>,
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
) -> Result<Vec<(Entry, Vec<MatchLabel>)>> {
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
    if let Some(field) = filter.sort_by {
        result.sort_by(|(a, _), (b, _)| {
            let ord = sort_cmp(a, b, field);
            if filter.sort_order == SortOrder::Desc { ord.reverse() } else { ord }
        });
    }
    Ok(result)
}

fn apply_filter_and_sort(entries: Vec<Entry>, filter: &EntryFilter) -> Result<Vec<(Entry, Vec<MatchLabel>)>> {
    let has_filter = filter.has_any_filter();
    let mut result = Vec::new();
    for entry in entries {
        let (include, labels) = filter.matches(&entry);
        if has_filter && !include {
            continue;
        }
        result.push((entry, labels));
    }
    if let Some(field) = filter.sort_by {
        result.sort_by(|(a, _), (b, _)| {
            let ord = sort_cmp(a, b, field);
            if filter.sort_order == SortOrder::Desc { ord.reverse() } else { ord }
        });
    }
    Ok(result)
}

fn sort_cmp(a: &Entry, b: &Entry, field: SortField) -> Ordering {
    match field {
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
        SortField::EventEnd   => cmp_opt(
            a.frontmatter.event.as_ref().map(|e| e.end),
            b.frontmatter.event.as_ref().map(|e| e.end),
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
/// All fields are optional. `title` is passed separately to [`create_entry`]
/// (required) and [`update_entry`] (optional).
#[derive(Debug, Default)]
pub struct EntryFields {
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

/// Create a new entry in `journal` with the given `title`, `body`, and `fields`.
///
/// Returns the path of the newly created file.
/// Fails with [`Error::EntryAlreadyExists`] if the destination already exists.
pub fn create_entry(
    journal: &Journal,
    title: &str,
    body: String,
    fields: EntryFields,
) -> Result<PathBuf> {
    let id = CarettaId::now_unix();
    let year = chrono::Local::now().year();

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
        parent_id: None,
        title: title.to_owned(),
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
pub fn update_entry(path: &Path, title: Option<String>, body: Option<String>, fields: EntryFields) -> Result<Option<PathBuf>> {
    let mut entry = read_entry(path)?;

    if let Some(t) = title {
        entry.frontmatter.title = t;
    }
    if let Some(b) = body {
        entry.body = b;
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

// ── prepare new (for editor workflow) ─────────────────────────────────────────

/// Create a new entry file with a frontmatter template in the journal's year directory.
///
/// The file is named `<id>.md` with required frontmatter pre-filled and optional
/// fields commented out.  The caller should open an editor on the returned path
/// and then call [`fix_entry`] to rename the file once the user has set a title.
pub fn prepare_new_entry(journal: &Journal) -> Result<PathBuf> {
    let id = CarettaId::now_unix();
    let year = chrono::Local::now().year();
    let now = chrono::Local::now().naive_local();
    let now_fmt = now.format("%Y-%m-%dT%H:%M");

    let dir = journal.root.join(year.to_string());
    std::fs::create_dir_all(&dir)?;

    let path = dir.join(format!("{id}.md"));

    let template = format!(
        "---\n\
         id: '{id}'\n\
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
            let conn = cache::open_cache(&journal)?;
            cache::sync_cache(&journal, &conn)?;
            cache::find_entry_by_id(&conn, id)
        }
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
fn entry_filename_from_frontmatter(id: CarettaId, fm: &Frontmatter) -> String {
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
