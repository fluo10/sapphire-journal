use anyhow::{bail, Context, Result};
use archelon_core::{
    entry_ref::EntryRef,
    journal::Journal,
    ops::{self, EntryFields as CoreEntryFields, EntryFilter, MatchLabel, SortField, SortOrder},
    period::{parse_datetime, parse_datetime_end, parse_period},
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

        /// AND filter: include only entries that have ALL specified tags.
        /// Comma-separated, e.g. work,urgent
        #[arg(long, value_name = "TAG[,...]", value_delimiter = ',', num_args = 1..)]
        tags: Option<Vec<String>>,

        /// Include overdue tasks (due in the past, not yet closed). OR'd with period filters.
        #[arg(long)]
        overdue: bool,

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
    /// Check whether an entry's frontmatter and filename are valid
    Check {
        /// Path to the entry file, or an ID / ID prefix
        entry: String,
    },
    /// Rename an entry file to match its frontmatter ID and title/slug
    Fix {
        /// Path to the entry file, or an ID / ID prefix
        entry: String,
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

impl From<EntryFields> for CoreEntryFields {
    fn from(f: EntryFields) -> Self {
        Self {
            title: f.title,
            slug: f.slug,
            tags: f.tags,
            task_due: f.task_due,
            task_status: f.task_status,
            task_closed_at: f.task_closed_at,
            event_start: f.event_start,
            event_end: f.event_end,
        }
    }
}

pub fn run(journal_dir: Option<&Path>, cmd: EntryCommand) -> Result<()> {
    match cmd {
        EntryCommand::List { path, period, task_due, event_start, event_end, created_at, updated_at, task_status, tags, overdue, sort_by, sort_order, json } => {
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
                tags: tags.unwrap_or_default(),
                overdue,
                sort_by: sort_by.as_deref()
                    .map(|s| s.parse::<SortField>().map_err(anyhow::Error::msg))
                    .transpose()?,
                sort_order: sort_order.parse::<SortOrder>().map_err(anyhow::Error::msg)?,
            };

            let entries = ops::list_entries(journal_dir, path.as_deref(), &filter)?;
            print_entries(&entries, filter.has_any_filter(), json)
        }
        EntryCommand::Show { entry } => show(&resolve_entry(journal_dir, &entry)?),
        EntryCommand::New { name, body, fields } => new(journal_dir, &name, body, fields),
        EntryCommand::Edit { entry } => edit(&resolve_entry(journal_dir, &entry)?),
        EntryCommand::Set { entry, fields } => set(journal_dir, &resolve_entry(journal_dir, &entry)?, fields),
        EntryCommand::Check { entry } => check(journal_dir, &entry),
        EntryCommand::Fix { entry } => fix(journal_dir, &entry),
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
                    .and_then(|t| t.status.as_deref())
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
            (Some(s), Some(e)) => println!("event:    {} – {}", s.format("%Y-%m-%d"), e.format("%Y-%m-%d")),
            (Some(s), None)    => println!("event:    from {}", s.format("%Y-%m-%d")),
            (None, Some(e))    => println!("event:    until {}", e.format("%Y-%m-%d")),
            (None, None)       => println!("event:    (no dates)"),
        }
    }
    println!();
    print!("{}", entry.body);
    Ok(())
}

// ── new ───────────────────────────────────────────────────────────────────────

fn new(journal_dir: Option<&Path>, name: &str, body: Option<String>, fields: EntryFields) -> Result<()> {
    let journal = open_journal(journal_dir)?;

    let body = match body {
        Some(b) => b,
        None => {
            let title = fields.title.as_deref().unwrap_or(name);
            let tags = fields.tags.as_deref().unwrap_or(&[]);
            prompt_editor(Some(title), tags)?
        }
    };

    let dest = ops::create_entry(&journal, name, body, fields.into())?;
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

fn set(journal_dir: Option<&Path>, path: &Path, fields: EntryFields) -> Result<()> {
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
    let _ = journal_dir; // reserved for future use
    if let Some(new_path) = ops::update_entry(path, fields.into())? {
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

fn fix(journal_dir: Option<&Path>, entry: &str) -> Result<()> {
    let path = resolve_entry(journal_dir, entry)?;
    match ops::fix_entry(&path)? {
        Some(new_path) => println!(
            "renamed: {} → {}",
            path.file_name().unwrap_or_default().to_string_lossy(),
            new_path.file_name().unwrap_or_default().to_string_lossy(),
        ),
        None => println!("ok: {} (already correct)", path.display()),
    }
    Ok(())
}

// ── remove ────────────────────────────────────────────────────────────────────

fn remove(journal_dir: Option<&Path>, entry: &str) -> Result<()> {
    let path = resolve_entry(journal_dir, entry)?;
    ops::remove_entry(&path)?;
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
