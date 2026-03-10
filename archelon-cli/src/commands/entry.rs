use anyhow::{bail, Context, Result};
use archelon_core::{
    cache,
    entry_ref::EntryRef,
    journal::Journal,
    ops::{self, EntryFields as CoreEntryFields, EntryFilter, FieldSelector, MatchLabel, SortField, SortOrder},
    period::{parse_datetime, parse_datetime_end, parse_period},
};
use chrono::NaiveDateTime;
use clap::{Args, Subcommand};
use std::{
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Subcommand)]
pub enum EntryCommand {
    /// List entries; optionally filter by period, field selectors, task status, and tags
    List {
        /// Time range to filter against. Without field selectors (--task-due, --event-span,
        /// --created-at, --updated-at) the period is applied to all timestamp fields (OR).
        /// Add field selectors to restrict matching to specific fields.
        ///
        /// PERIOD formats: today | this_week | this_month | none |
        /// YYYY-MM-DD | YYYY-MM-DD,YYYY-MM-DD | YYYY-MM-DDTHH:MM,YYYY-MM-DDTHH:MM
        #[arg(long, value_name = "PERIOD")]
        period: Option<String>,

        /// Restrict --period to task due date.
        /// Without --period: include entries that have a task_due set.
        #[arg(long)]
        task_due: bool,

        /// Restrict --period to event span (overlap semantics).
        /// Without --period: include entries that have an event set.
        #[arg(long)]
        event_span: bool,

        /// Restrict --period to created_at timestamp.
        #[arg(long)]
        created_at: bool,

        /// Restrict --period to updated_at timestamp.
        #[arg(long)]
        updated_at: bool,

        /// Filter by task status (AND with timestamp filters).
        /// Comma-separated for multiple values, e.g. open,in_progress
        #[arg(long, value_name = "STATUS[,...]", value_delimiter = ',', num_args = 1..)]
        task_status: Option<Vec<String>>,

        /// AND filter: include only entries that have ALL specified tags.
        /// Comma-separated, e.g. work,urgent
        #[arg(long, value_name = "TAG[,...]", value_delimiter = ',', num_args = 1..)]
        tags: Option<Vec<String>>,

        /// Include overdue tasks (due in the past, not yet closed). OR'd with period filters.
        #[arg(long)]
        overdue: bool,

        /// Include tasks that have been started (started_at set) and not yet closed.
        /// When combined with --period, only tasks started on or before the period end are included.
        /// OR'd with period filters.
        #[arg(long)]
        task_started: bool,

        /// Sort results by a field.
        /// Values: id | title | task_status | created_at | updated_at | task_due | event_start | event_end
        #[arg(long, value_name = "FIELD")]
        sort_by: Option<String>,

        /// Sort direction: asc (default) or desc
        #[arg(long, value_name = "ORDER", default_value = "asc")]
        sort_order: String,

        /// Output all matching entries as JSON (metadata + body) for AI/machine consumption
        #[arg(long)]
        json: bool,
    },
    /// Show the contents of an entry
    Show {
        /// Path to the entry file, or an ID / ID prefix
        entry: String,
    },
    /// Create a new entry with an optional body.
    New {
        #[command(flatten)]
        fields: EntryFields,
    },
    /// Open an entry in $EDITOR. Without an argument, or with --new, creates a new entry.
    Edit {
        /// Path to the entry file, or an ID / ID prefix
        entry: Option<String>,

        /// Create a new entry and open it in $EDITOR with a pre-filled frontmatter template
        #[arg(long)]
        new: bool,
    },
    /// Update frontmatter fields without opening an editor
    Set {
        /// Path to the entry file, or an ID / ID prefix
        entry: String,

        #[command(flatten)]
        fields: EntryFields,
    },
    /// Check whether an entry's frontmatter and filename are valid
    Check {
        /// Path to the entry file, or an ID / ID prefix
        entry: String,
    },
    /// Normalize an entry: sync closed_at, rename to match frontmatter, and optionally refresh updated_at
    Fix {
        /// Path to the entry file, or an ID / ID prefix
        entry: String,

        /// Also update updated_at to the current time
        #[arg(long)]
        touch: bool,
    },
    /// Print the absolute path of an entry, or create a new template and print its path.
    /// Intended for editor integrations that need a file path without opening an editor.
    Path {
        /// File path to the entry, or an ID / ID prefix.
        /// Required unless --new is given.
        #[arg(required_unless_present = "new")]
        entry: Option<String>,

        /// Create a new entry template file and print its path instead of resolving an existing entry
        #[arg(long)]
        new: bool,
    },
    /// Delete an entry file
    Remove {
        /// Path to the entry file, or an ID / ID prefix
        entry: String,
    },
}

/// Frontmatter fields shared between `entry new` and `entry set` (clap-aware).
///
/// After parsing this is converted into [`archelon_core::ops::EntryFields`]
/// and passed to the core operation functions.
#[derive(Args)]
pub struct EntryFields {
    /// Title of the entry — written into the frontmatter and used to generate the filename slug
    #[arg(long, short)]
    pub title: Option<String>,

    /// Body content (Markdown). For `entry set`, replaces the existing body.
    #[arg(long, short)]
    pub body: Option<String>,

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

    /// Task start date/time; set automatically when status → in_progress
    #[arg(long, value_name = "DATETIME", value_parser = parse_datetime)]
    pub task_started_at: Option<NaiveDateTime>,

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

impl From<EntryFields> for CoreEntryFields {
    fn from(f: EntryFields) -> Self {
        Self {
            slug: f.slug,
            tags: f.tags,
            task_due: f.task_due,
            task_status: f.task_status,
            task_started_at: f.task_started_at,
            task_closed_at: f.task_closed_at,
            event_start: f.event_start,
            event_end: f.event_end,
        }
    }
}

pub fn run(journal_dir: Option<&Path>, cmd: EntryCommand) -> Result<()> {
    match cmd {
        EntryCommand::List { period, task_due, event_span, created_at, updated_at, task_status, tags, overdue, task_started, sort_by, sort_order, json } => {
            // Resolve week_start from journal config (needed for this_week parsing)
            let week_start = open_journal(journal_dir)
                .and_then(|j| j.config().map_err(Into::into))
                .map(|c| c.journal.week_start)
                .unwrap_or_default();

            let parse = |s: &str| parse_period(s, week_start).map_err(anyhow::Error::msg);

            let filter = EntryFilter {
                period: period.as_deref().map(parse).transpose()?,
                fields: FieldSelector { task_due, event_span, created_at, updated_at },
                task_status: task_status.unwrap_or_default(),
                tags: tags.unwrap_or_default(),
                overdue,
                task_started,
                sort_by: sort_by.as_deref()
                    .map(|s| s.parse::<SortField>().map_err(anyhow::Error::msg))
                    .transpose()?,
                sort_order: sort_order.parse::<SortOrder>().map_err(anyhow::Error::msg)?,
            };

            let entries = ops::list_entries(journal_dir, &filter)?;
            print_entries(&entries, filter.has_any_filter(), json)
        }
        EntryCommand::Show { entry } => show(&resolve_entry(journal_dir, &entry)?),
        EntryCommand::New { fields } => new(journal_dir, fields),
        EntryCommand::Edit { entry, new } => {
            if new {
                edit_new(journal_dir)
            } else if let Some(e) = entry {
                edit(&resolve_entry(journal_dir, &e)?, journal_dir)
            } else {
                bail!("specify an entry or use --new to create one")
            }
        }
        EntryCommand::Set { entry, fields } => set(journal_dir, &resolve_entry(journal_dir, &entry)?, fields),
        EntryCommand::Check { entry } => check(journal_dir, &entry),
        EntryCommand::Fix { entry, touch } => fix(journal_dir, &entry, touch),
        EntryCommand::Path { entry, new } => entry_path(journal_dir, entry.as_deref(), new),
        EntryCommand::Remove { entry } => remove(journal_dir, &entry),
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

// ── list output ───────────────────────────────────────────────────────────────

fn print_entries(
    entries: &[(archelon_core::entry::Entry, Vec<MatchLabel>)],
    has_filter: bool,
    json: bool,
) -> Result<()> {
    if json {
        let records: Vec<serde_json::Value> = entries
            .iter()
            .map(|(entry, labels)| {
                let mut v = serde_json::json!({
                    "id": entry.id().to_string(),
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

    let rows: Vec<(String, String, String)> = entries
        .iter()
        .map(|(entry, labels)| {
            let id = entry.id().to_string();
            let status = if has_filter && !labels.is_empty() {
                labels.iter().map(|l| l.as_str()).collect::<Vec<_>>().join(",")
            } else {
                entry
                    .frontmatter
                    .task
                    .as_ref()
                    .map(|t| t.status.as_str())
                    .unwrap_or("")
                    .to_owned()
            };
            (id, status, entry.title().to_owned())
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
    use archelon_core::parser::read_entry;

    let entry = read_entry(path)?;
    let fm = &entry.frontmatter;

    println!("# {}", entry.title());
    println!("created:  {}", fm.created_at.format("%Y-%m-%dT%H:%M"));
    println!("updated:  {}", fm.updated_at.format("%Y-%m-%dT%H:%M"));
    if !fm.tags.is_empty() {
        println!("tags:     {}", fm.tags.join(", "));
    }
    if let Some(task) = &fm.task {
        let status = task.status.as_str();
        match task.due {
            Some(d) => println!("task:     {status} (due {})", d.format("%Y-%m-%d")),
            None => println!("task:     {status}"),
        }
        if let Some(sa) = task.started_at {
            println!("started:  {}", sa.format("%Y-%m-%dT%H:%M"));
        }
        if let Some(ca) = task.closed_at {
            println!("closed:   {}", ca.format("%Y-%m-%dT%H:%M"));
        }
    }
    if let Some(event) = &fm.event {
        println!("event:    {} – {}", event.start.format("%Y-%m-%d"), event.end.format("%Y-%m-%d"));
    }
    println!();
    print!("{}", entry.body);
    Ok(())
}

// ── new ───────────────────────────────────────────────────────────────────────

fn new(journal_dir: Option<&Path>, fields: EntryFields) -> Result<()> {
    let journal = open_journal(journal_dir)?;
    let title = fields.title.clone().unwrap_or_default();
    let body = fields.body.clone().unwrap_or_default();
    let dest = ops::create_entry(&journal, &title, body, fields.into())?;
    if let Ok(conn) = cache::open_cache(&journal) {
        let _ = cache::upsert_entry_from_path(&conn, &dest);
    }
    println!("created: {}", dest.display());
    Ok(())
}

// ── edit ──────────────────────────────────────────────────────────────────────

fn edit(path: &Path, journal_dir: Option<&Path>) -> Result<()> {
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
    let final_path = match ops::fix_entry(path, true)? {
        Some(new_path) => { println!("updated: {}", new_path.display()); new_path }
        None           => { println!("updated: {}", path.display()); path.to_path_buf() }
    };
    if let Ok(journal) = open_journal(journal_dir) {
        if let Ok(conn) = cache::open_cache(&journal) {
            let _ = cache::upsert_entry_from_path(&conn, &final_path);
        }
    }
    Ok(())
}

fn edit_new(journal_dir: Option<&Path>) -> Result<()> {
    let journal = open_journal(journal_dir)?;
    let path = ops::prepare_new_entry(&journal)?;

    let editor = resolve_editor();
    let status = Command::new(&editor)
        .arg(&path)
        .status()
        .with_context(|| format!("failed to launch editor `{editor}`"))?;
    if !status.success() {
        let _ = std::fs::remove_file(&path);
        bail!("editor exited with non-zero status");
    }

    let final_path = match ops::fix_entry(&path, true)? {
        Some(new_path) => { println!("created: {}", new_path.display()); new_path }
        None           => { println!("created: {}", path.display()); path }
    };
    if let Ok(conn) = cache::open_cache(&journal) {
        let _ = cache::upsert_entry_from_path(&conn, &final_path);
    }
    Ok(())
}

// ── set ───────────────────────────────────────────────────────────────────────

fn set(journal_dir: Option<&Path>, path: &Path, fields: EntryFields) -> Result<()> {
    if fields.title.is_none()
        && fields.body.is_none()
        && fields.slug.is_none()
        && fields.tags.is_none()
        && fields.task_due.is_none()
        && fields.task_status.is_none()
        && fields.task_started_at.is_none()
        && fields.task_closed_at.is_none()
        && fields.event_start.is_none()
        && fields.event_end.is_none()
    {
        bail!("nothing to update — specify at least one field");
    }
    let _ = journal_dir; // reserved for future use
    let title = fields.title.clone();
    let body = fields.body.clone();
    let new_path_opt = ops::update_entry(path, title, body, fields.into())?;
    let final_path = new_path_opt.as_deref().unwrap_or(path);
    if let Ok(journal) = open_journal(journal_dir) {
        if let Ok(conn) = cache::open_cache(&journal) {
            let _ = cache::upsert_entry_from_path(&conn, final_path);
        }
    }
    if let Some(new_path) = new_path_opt {
        println!("updated and renamed: {}", new_path.display());
    } else {
        println!("updated: {}", path.display());
    }
    Ok(())
}

// ── check ─────────────────────────────────────────────────────────────────────

fn check(journal_dir: Option<&Path>, entry: &str) -> Result<()> {
    let path = resolve_entry(journal_dir, entry)?;
    let issues = ops::check_entry(&path)?;
    if issues.is_empty() {
        println!("ok: {}", path.display());
    } else {
        for issue in &issues {
            println!("{}: {}", path.display(), issue.as_str());
        }
    }
    Ok(())
}

// ── fix ───────────────────────────────────────────────────────────────────────

fn fix(journal_dir: Option<&Path>, entry: &str, touch: bool) -> Result<()> {
    let path = resolve_entry(journal_dir, entry)?;
    match ops::fix_entry(&path, touch)? {
        Some(new_path) => println!(
            "renamed: {} → {}",
            path.file_name().unwrap_or_default().to_string_lossy(),
            new_path.file_name().unwrap_or_default().to_string_lossy(),
        ),
        None => println!("ok: {} (already correct)", path.display()),
    }
    Ok(())
}

// ── path ──────────────────────────────────────────────────────────────────────

fn entry_path(journal_dir: Option<&Path>, entry: Option<&str>, new: bool) -> Result<()> {
    if new {
        let journal = open_journal(journal_dir)?;
        let path = ops::prepare_new_entry(&journal)?;
        println!("{}", path.display());
    } else {
        let path = resolve_entry(journal_dir, entry.unwrap())?;
        println!("{}", path.display());
    }
    Ok(())
}

// ── remove ────────────────────────────────────────────────────────────────────

fn remove(journal_dir: Option<&Path>, entry: &str) -> Result<()> {
    let path = resolve_entry(journal_dir, entry)?;
    ops::remove_entry(&path)?;
    // Keep the cache consistent after explicit deletion.
    if let Ok(journal) = open_journal(journal_dir) {
        if let Ok(conn) = archelon_core::cache::open_cache(&journal) {
            let _ = archelon_core::cache::remove_from_cache(&conn, &path);
        }
    }
    println!("removed: {}", path.display());
    Ok(())
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn resolve_entry(journal_dir: Option<&Path>, entry: &str) -> Result<PathBuf> {
    ops::resolve_entry(&EntryRef::parse(entry), journal_dir).map_err(Into::into)
}

fn resolve_editor() -> String {
    std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vi".into())
}

