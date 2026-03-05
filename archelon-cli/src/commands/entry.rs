use anyhow::{bail, Context, Result};
use archelon_core::{
    entry::{Entry, EventMeta, Frontmatter, TaskMeta},
    journal::{is_managed_filename, Journal, new_entry_path},
    parser::{read_entry, render_entry, write_entry},
    period::{parse_datetime, parse_datetime_end, parse_period, Period},
};
use chrono::NaiveDateTime;
use clap::{Args, Subcommand};
use std::{
    io::Write as _,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Subcommand)]
pub enum EntryCommand {
    /// List entries; optionally filter by per-field period conditions and/or task status
    List {
        /// Directory to search (defaults to journal root, then current directory)
        path: Option<PathBuf>,

        /// Convenience filter applied to all timestamp fields simultaneously (OR across fields).
        /// Equivalent to setting --task-due, --event-start, --event-end, --created-at, --updated-at
        /// to the same period.
        ///
        /// PERIOD formats: today | this_week | this_month | none |
        /// YYYY-MM-DD | YYYY-MM-DD,YYYY-MM-DD | YYYY-MM-DDTHH:MM,YYYY-MM-DDTHH:MM
        #[arg(long, value_name = "PERIOD")]
        period: Option<String>,

        /// Filter by task due date (PERIOD format, see --period)
        #[arg(long, value_name = "PERIOD")]
        task_due: Option<String>,

        /// Filter by event start date (PERIOD format, see --period)
        #[arg(long, value_name = "PERIOD")]
        event_start: Option<String>,

        /// Filter by event end date (PERIOD format, see --period)
        #[arg(long, value_name = "PERIOD")]
        event_end: Option<String>,

        /// Filter by created_at timestamp (PERIOD format, see --period)
        #[arg(long, value_name = "PERIOD")]
        created_at: Option<String>,

        /// Filter by updated_at timestamp (PERIOD format, see --period)
        #[arg(long, value_name = "PERIOD")]
        updated_at: Option<String>,

        /// Filter by task status (AND with timestamp filters).
        /// Comma-separated for multiple values, e.g. open,in_progress
        #[arg(long, value_name = "STATUS[,...]", value_delimiter = ',', num_args = 1..)]
        task_status: Option<Vec<String>>,

        /// Output all matching entries as JSON (metadata + body) for AI/machine consumption
        #[arg(long)]
        json: bool,
    },
    /// Show the contents of an entry
    Show {
        /// Path to the entry file, or an ID / ID prefix
        entry: String,
    },
    /// Create a new entry.
    /// Without --body, opens $EDITOR (like `git commit` without -m).
    New {
        /// Name of the entry — used as the title and to generate the filename slug
        name: String,

        /// Inline body content — skips the editor (like git commit -m)
        #[arg(long, short)]
        body: Option<String>,

        #[command(flatten)]
        fields: EntryFields,
    },
    /// Open an entry in $EDITOR
    Edit {
        /// Path to the entry file, or an ID / ID prefix
        entry: String,
    },
    /// Update frontmatter fields without opening an editor
    Set {
        /// Path to the entry file, or an ID / ID prefix
        entry: String,

        #[command(flatten)]
        fields: EntryFields,
    },
}

/// Frontmatter fields shared between `entry new` and `entry set`.
#[derive(Args)]
pub struct EntryFields {
    /// Title written into the frontmatter
    #[arg(long, short)]
    pub title: Option<String>,

    /// Slug override in the frontmatter
    #[arg(long)]
    pub slug: Option<String>,

    /// Tags (comma-separated); pass with no value to clear all tags
    #[arg(long, short = 'T', num_args = 0.., value_delimiter = ',')]
    pub tags: Option<Vec<String>>,

    /// Task due date/time (YYYY-MM-DD or YYYY-MM-DDTHH:MM; date-only = 23:59)
    #[arg(long, value_name = "DATETIME", value_parser = parse_datetime_end)]
    pub task_due: Option<NaiveDateTime>,

    /// Task status (open | in_progress | done | cancelled | archived)
    #[arg(long)]
    pub task_status: Option<String>,

    /// Task close date/time; set automatically when status → done/cancelled/archived
    #[arg(long, value_name = "DATETIME", value_parser = parse_datetime)]
    pub task_closed_at: Option<NaiveDateTime>,

    /// Event start date/time (YYYY-MM-DD or YYYY-MM-DDTHH:MM)
    #[arg(long, value_name = "DATETIME", value_parser = parse_datetime)]
    pub event_start: Option<NaiveDateTime>,

    /// Event end date/time (YYYY-MM-DD or YYYY-MM-DDTHH:MM; date-only = 23:59)
    #[arg(long, value_name = "DATETIME", value_parser = parse_datetime_end)]
    pub event_end: Option<NaiveDateTime>,
}

pub fn run(journal_dir: Option<&Path>, cmd: EntryCommand) -> Result<()> {
    match cmd {
        EntryCommand::List { path, period, task_due, event_start, event_end, created_at, updated_at, task_status, json } => {
            // Resolve week_start from journal config (needed for this_week parsing)
            let week_start = open_journal(journal_dir)
                .and_then(|j| j.config().map_err(Into::into))
                .map(|c| c.journal.week_start)
                .unwrap_or_default();

            let parse = |s: &str| parse_period(s, week_start).map_err(anyhow::Error::msg);

            let filter = EntryFilter {
                period: period.as_deref().map(parse).transpose()?,
                task_due: task_due.as_deref().map(parse).transpose()?,
                event_start: event_start.as_deref().map(parse).transpose()?,
                event_end: event_end.as_deref().map(parse).transpose()?,
                created_at: created_at.as_deref().map(parse).transpose()?,
                updated_at: updated_at.as_deref().map(parse).transpose()?,
                task_status: task_status.unwrap_or_default(),
            };

            list(journal_dir, path.as_deref(), filter, json)
        }
        EntryCommand::Show { entry } => show(&resolve_entry(journal_dir, &entry)?),
        EntryCommand::New { name, body, fields } => new(journal_dir, &name, body, fields),
        EntryCommand::Edit { entry } => edit(&resolve_entry(journal_dir, &entry)?),
        EntryCommand::Set { entry, fields } => set(&resolve_entry(journal_dir, &entry)?, fields),
    }
}

fn open_journal(journal_dir: Option<&Path>) -> Result<Journal> {
    match journal_dir {
        Some(dir) => Journal::from_root(dir.to_path_buf())
            .context("not an archelon journal — run `archelon init` to initialize one"),
        None => Journal::find()
            .context("not in an archelon journal — run `archelon init` to initialize one"),
    }
}

// ── filter ────────────────────────────────────────────────────────────────────

struct EntryFilter {
    /// Shortcut: applies the same period to all timestamp fields simultaneously (OR across fields)
    period: Option<Period>,
    task_due: Option<Period>,
    event_start: Option<Period>,
    event_end: Option<Period>,
    created_at: Option<Period>,
    updated_at: Option<Period>,
    /// AND condition: entry must have a task whose status is one of these values
    task_status: Vec<String>,
}

impl EntryFilter {
    fn has_timestamp_filter(&self) -> bool {
        self.period.is_some()
            || self.task_due.is_some()
            || self.event_start.is_some()
            || self.event_end.is_some()
            || self.created_at.is_some()
            || self.updated_at.is_some()
    }

    /// Returns `(include, labels)`.
    ///
    /// Timestamp filters are ORed: any one matching field is sufficient.
    /// `task_status` is ANDed on top: the entry must also satisfy the status filter.
    fn matches(&self, entry: &Entry) -> (bool, Vec<MatchLabel>) {
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

            // --period applies to all fields
            if let Some(p) = &self.period {
                if p.matches(task_due_val) {
                    labels.push(MatchLabel::TaskDue);
                }
                if p.matches(event_start_val) {
                    labels.push(MatchLabel::EventStart);
                }
                if p.matches(event_end_val) {
                    labels.push(MatchLabel::EventEnd);
                }
                if p.matches(created_val) {
                    labels.push(MatchLabel::CreatedAt);
                }
                if p.matches(updated_val) {
                    labels.push(MatchLabel::UpdatedAt);
                }
            }

            // per-field filters (may add duplicate labels; dedup below)
            check!(self.task_due, task_due_val, MatchLabel::TaskDue);
            check!(self.event_start, event_start_val, MatchLabel::EventStart);
            check!(self.event_end, event_end_val, MatchLabel::EventEnd);
            check!(self.created_at, created_val, MatchLabel::CreatedAt);
            check!(self.updated_at, updated_val, MatchLabel::UpdatedAt);

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

// ── list ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MatchLabel {
    TaskDue,
    EventStart,
    EventEnd,
    CreatedAt,
    UpdatedAt,
}

impl MatchLabel {
    fn as_str(self) -> &'static str {
        match self {
            MatchLabel::TaskDue => "TASK_DUE",
            MatchLabel::EventStart => "EVENT_START",
            MatchLabel::EventEnd => "EVENT_END",
            MatchLabel::CreatedAt => "CREATED",
            MatchLabel::UpdatedAt => "UPDATED",
        }
    }
}

fn list(
    journal_dir: Option<&Path>,
    path: Option<&Path>,
    filter: EntryFilter,
    json: bool,
) -> Result<()> {
    let paths = collect_entries(journal_dir, path)?;
    let has_filter = filter.has_timestamp_filter() || !filter.task_status.is_empty();

    let mut filtered: Vec<(Entry, Vec<MatchLabel>)> = Vec::new();

    for path in &paths {
        if !is_managed_filename(path) {
            continue;
        }
        let entry = match read_entry(path) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("warn: {} — {e}", path.display());
                continue;
            }
        };

        let (include, labels) = filter.matches(&entry);
        if has_filter && !include {
            continue;
        }

        filtered.push((entry, labels));
    }

    if json {
        let records: Vec<serde_json::Value> = filtered
            .iter()
            .map(|(entry, labels)| {
                let mut v = serde_json::json!({
                    "id": entry.id().map(|id| id.to_string()),
                    "path": entry.path.display().to_string(),
                    "title": entry.title(),
                    "slug": entry.frontmatter.slug,
                    "created_at": entry.frontmatter.created_at,
                    "updated_at": entry.frontmatter.updated_at,
                    "tags": entry.frontmatter.tags,
                    "task": entry.frontmatter.task,
                    "event": entry.frontmatter.event,
                    "body": entry.body,
                });
                if has_filter {
                    v["match_labels"] = serde_json::json!(
                        labels.iter().map(|l| l.as_str()).collect::<Vec<_>>()
                    );
                }
                v
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&records)?);
        return Ok(());
    }

    // Table output
    let rows: Vec<(String, String, String)> = filtered
        .iter()
        .map(|(entry, labels)| {
            let id = entry.id().map(|id| id.to_string()).unwrap_or_default();
            let status = if has_filter && !labels.is_empty() {
                labels.iter().map(|l| l.as_str()).collect::<Vec<_>>().join(",")
            } else {
                entry
                    .frontmatter
                    .task
                    .as_ref()
                    .and_then(|t| t.status.as_deref())
                    .unwrap_or("")
                    .to_owned()
            };
            let title = entry.title().to_owned();
            (id, status, title)
        })
        .collect();

    if rows.is_empty() {
        return Ok(());
    }

    let id_w = rows.iter().map(|(id, _, _)| id.len()).max().unwrap_or(7);
    let status_w = rows.iter().map(|(_, s, _)| s.len()).max().unwrap_or(0);

    for (id, status, title) in &rows {
        println!("{:<id_w$}  {:<status_w$}  {title}", id, status);
    }

    Ok(())
}

// ── show ──────────────────────────────────────────────────────────────────────

fn show(path: &Path) -> Result<()> {
    let entry = read_entry(path)?;
    let fm = &entry.frontmatter;

    println!("# {}", entry.title());

    if let Some(ts) = fm.created_at {
        println!("created:  {}", ts.format("%Y-%m-%dT%H:%M"));
    }
    if let Some(ts) = fm.updated_at {
        println!("updated:  {}", ts.format("%Y-%m-%dT%H:%M"));
    }
    if !fm.tags.is_empty() {
        println!("tags:     {}", fm.tags.join(", "));
    }
    if let Some(task) = &fm.task {
        let status = task.status.as_deref().unwrap_or("open");
        match task.due {
            Some(d) => println!("task:     {status} (due {})", d.format("%Y-%m-%d")),
            None => println!("task:     {status}"),
        }
        if let Some(ca) = task.closed_at {
            println!("closed:   {}", ca.format("%Y-%m-%dT%H:%M"));
        }
    }
    if let Some(event) = &fm.event {
        match (event.start, event.end) {
            (Some(s), Some(e)) => {
                println!("event:    {} – {}", s.format("%Y-%m-%d"), e.format("%Y-%m-%d"))
            }
            (Some(s), None) => println!("event:    from {}", s.format("%Y-%m-%d")),
            (None, Some(e)) => println!("event:    until {}", e.format("%Y-%m-%d")),
            (None, None) => println!("event:    (no dates)"),
        }
    }

    println!();
    print!("{}", entry.body);

    Ok(())
}

// ── new ───────────────────────────────────────────────────────────────────────

fn new(journal_dir: Option<&Path>, name: &str, body: Option<String>, fields: EntryFields) -> Result<()> {
    let fm_title = fields.title.or_else(|| Some(name.to_owned()));
    let tags = fields.tags.unwrap_or_default();
    let journal = open_journal(journal_dir)?;
    let dest = journal.root.join(new_entry_path(name));

    if dest.exists() {
        bail!("{} already exists", dest.display());
    }

    let body = match body {
        Some(b) => b,
        None => prompt_editor(fm_title.as_deref(), &tags)?,
    };

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

    std::fs::create_dir_all(dest.parent().unwrap())
        .with_context(|| format!("failed to create directory for {}", dest.display()))?;
    std::fs::write(&dest, render_entry(&entry))
        .with_context(|| format!("failed to write {}", dest.display()))?;

    println!("created: {}", dest.display());
    Ok(())
}

// ── edit ──────────────────────────────────────────────────────────────────────

fn edit(path: &Path) -> Result<()> {
    if !path.exists() {
        bail!("{} does not exist", path.display());
    }

    let editor = resolve_editor();
    let status = Command::new(&editor)
        .arg(path)
        .status()
        .with_context(|| format!("failed to launch editor `{editor}`"))?;

    if !status.success() {
        bail!("editor exited with non-zero status");
    }

    Ok(())
}

// ── set ───────────────────────────────────────────────────────────────────────

fn set(path: &Path, fields: EntryFields) -> Result<()> {
    if fields.title.is_none()
        && fields.slug.is_none()
        && fields.tags.is_none()
        && fields.task_due.is_none()
        && fields.task_status.is_none()
        && fields.task_closed_at.is_none()
        && fields.event_start.is_none()
        && fields.event_end.is_none()
    {
        bail!("nothing to update — specify at least one field");
    }

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
            // Auto-set closed_at when transitioning to inactive (unless already set or overridden)
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
    println!("updated: {}", path.display());
    Ok(())
}

// ── helpers ───────────────────────────────────────────────────────────────────

/// Resolve a user-supplied `entry` argument to a concrete file path.
///
/// Resolution order:
/// 1. If the argument is an existing file path, return it as-is.
/// 2. Otherwise treat it as an ID prefix and search the current journal.
fn resolve_entry(journal_dir: Option<&Path>, entry: &str) -> Result<PathBuf> {
    let p = Path::new(entry);
    if p.exists() {
        return Ok(p.to_path_buf());
    }

    let journal = open_journal(journal_dir)?;
    journal.find_entry_by_id(entry).map_err(Into::into)
}

/// Collect `.md` files for the list command.
/// If path is given, scan only that directory.
/// Otherwise, use journal root + year subdirs; fall back to ".".
fn collect_entries(journal_dir: Option<&Path>, path: Option<&Path>) -> Result<Vec<PathBuf>> {
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
        return Journal::from_root(dir.to_path_buf())
            .context("not an archelon journal")?
            .collect_entries()
            .map_err(Into::into);
    }

    if let Ok(journal) = Journal::find() {
        return journal.collect_entries().map_err(Into::into);
    }

    let mut paths: Vec<_> = std::fs::read_dir(".")?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("md"))
        .collect();
    paths.sort();
    Ok(paths)
}

fn resolve_editor() -> String {
    std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vi".into())
}

/// Open a temp file in $EDITOR and return the body the user typed.
fn prompt_editor(title: Option<&str>, tags: &[String]) -> Result<String> {
    let editor = resolve_editor();

    let mut template =
        "# archelon: Write your entry below. Lines starting with '# archelon:' are ignored.\n"
            .to_owned();
    if let Some(t) = title {
        template.push_str(&format!("# archelon: title = {t}\n"));
    }
    if !tags.is_empty() {
        template.push_str(&format!("# archelon: tags  = {}\n", tags.join(", ")));
    }
    template.push('\n');

    let mut tmp = tempfile::Builder::new()
        .prefix("archelon-")
        .suffix(".md")
        .tempfile()?;
    tmp.write_all(template.as_bytes())?;
    tmp.flush()?;

    let status = Command::new(&editor)
        .arg(tmp.path())
        .status()
        .with_context(|| format!("failed to launch editor `{editor}`"))?;

    if !status.success() {
        bail!("editor exited with non-zero status");
    }

    let content = std::fs::read_to_string(tmp.path())?;
    let body: String = content
        .lines()
        .filter(|l| !l.starts_with("# archelon:"))
        .collect::<Vec<_>>()
        .join("\n");
    let body = body.trim_start_matches('\n').to_owned();

    if body.is_empty() {
        bail!("aborting: empty entry");
    }

    Ok(body)
}
