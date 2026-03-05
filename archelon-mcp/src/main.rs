use std::path::{Path, PathBuf};

use anyhow::Context as _;
use archelon_core::{
    entry::{Entry, EventMeta, Frontmatter, TaskMeta},
    journal::{is_managed_filename, Journal, WeekStart, new_entry_path},
    parser::{read_entry, render_entry, write_entry},
};
use chrono::{Datelike as _, Duration, NaiveDate, NaiveDateTime};
use rmcp::{
    ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::*,
    schemars, tool, tool_handler, tool_router,
    transport::stdio,
};
use serde::Deserialize;

// ── server struct ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct ArchelonServer {
    journal_dir: Option<PathBuf>,
    tool_router: ToolRouter<Self>,
}

impl ArchelonServer {
    fn new(journal_dir: Option<PathBuf>) -> Self {
        Self {
            journal_dir,
            tool_router: Self::tool_router(),
        }
    }

    fn open_journal(&self) -> anyhow::Result<Journal> {
        match &self.journal_dir {
            Some(dir) => Journal::from_root(dir.clone())
                .context("not an archelon journal — run `journal_init` first"),
            None => Journal::find()
                .context("not in an archelon journal — run `journal_init` first"),
        }
    }

    fn resolve_entry(&self, entry: &str) -> anyhow::Result<PathBuf> {
        let p = Path::new(entry);
        if p.exists() {
            return Ok(p.to_path_buf());
        }
        let journal = self.open_journal()?;
        journal.find_entry_by_id(entry).map_err(Into::into)
    }
}

// ── parameter structs ─────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct InitParams {
    /// Directory to initialize (created if it does not exist). Defaults to current directory.
    path: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct EntryListParams {
    /// Start of the date range, inclusive (YYYY-MM-DD)
    date_start: Option<String>,
    /// End of the date range, inclusive (YYYY-MM-DD)
    date_end: Option<String>,
    /// Alias for date_start + date_end set to the same day (YYYY-MM-DD)
    date: Option<String>,
    /// Filter to today's entries
    today: Option<bool>,
    /// Filter to the current week
    this_week: Option<bool>,
    /// Filter to the current calendar month
    this_month: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct EntryShowParams {
    /// File path to the entry, or an ID / ID prefix
    entry: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct EntryNewParams {
    /// Name of the entry — used as the title and to generate the filename slug
    name: String,
    /// Body content (Markdown)
    body: String,
    /// Title written into the frontmatter (defaults to `name`)
    title: Option<String>,
    /// Slug override in the frontmatter
    slug: Option<String>,
    /// Tags as comma-separated string (e.g. "work,project")
    tags: Option<String>,
    /// Task due date/time (YYYY-MM-DD or YYYY-MM-DDTHH:MM)
    task_due: Option<String>,
    /// Task status (open | in_progress | done | cancelled | archived)
    task_status: Option<String>,
    /// Task close date/time
    task_closed_at: Option<String>,
    /// Event start date/time (YYYY-MM-DD or YYYY-MM-DDTHH:MM)
    event_start: Option<String>,
    /// Event end date/time (YYYY-MM-DD or YYYY-MM-DDTHH:MM)
    event_end: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct EntrySetParams {
    /// File path to the entry, or an ID / ID prefix
    entry: String,
    /// New title
    title: Option<String>,
    /// New slug override
    slug: Option<String>,
    /// New tags as comma-separated string. Pass empty string to clear all tags.
    tags: Option<String>,
    /// Task due date/time (YYYY-MM-DD or YYYY-MM-DDTHH:MM)
    task_due: Option<String>,
    /// Task status (open | in_progress | done | cancelled | archived)
    task_status: Option<String>,
    /// Task close date/time
    task_closed_at: Option<String>,
    /// Event start date/time (YYYY-MM-DD or YYYY-MM-DDTHH:MM)
    event_start: Option<String>,
    /// Event end date/time (YYYY-MM-DD or YYYY-MM-DDTHH:MM)
    event_end: Option<String>,
}

// ── tool implementations ──────────────────────────────────────────────────────

#[tool_router]
impl ArchelonServer {
    #[tool(description = "Initialize a new archelon journal in the given directory (defaults to current directory)")]
    fn journal_init(&self, Parameters(p): Parameters<InitParams>) -> Result<String, String> {
        (|| -> anyhow::Result<String> {
            let target = p.path.as_deref().unwrap_or(".");
            let target = Path::new(target);

            if !target.exists() {
                std::fs::create_dir_all(target)
                    .with_context(|| format!("failed to create directory {}", target.display()))?;
            }

            let archelon_dir = target.join(".archelon");
            if archelon_dir.exists() {
                anyhow::bail!(
                    "journal already initialized at {}",
                    target.canonicalize()?.display()
                );
            }

            std::fs::create_dir(&archelon_dir).context("failed to create .archelon directory")?;

            let tz = detect_timezone();
            std::fs::write(
                archelon_dir.join("config.toml"),
                format!("[journal]\ntimezone = \"{tz}\"\n"),
            )
            .context("failed to write .archelon/config.toml")?;
            std::fs::write(archelon_dir.join(".gitignore"), "cache/\n")
                .context("failed to write .archelon/.gitignore")?;

            Ok(format!(
                "initialized archelon journal in {}",
                target.canonicalize()?.display()
            ))
        })()
        .map_err(|e| e.to_string())
    }

    #[tool(description = "List journal entries as JSON, optionally filtered by date range")]
    fn entry_list(&self, Parameters(p): Parameters<EntryListParams>) -> Result<String, String> {
        (|| -> anyhow::Result<String> {
            let week_start = if p.this_week.unwrap_or(false) {
                self.open_journal()
                    .and_then(|j| j.config().map_err(Into::into))
                    .map(|c| c.journal.week_start)
                    .unwrap_or_default()
            } else {
                WeekStart::default()
            };

            let date = p.date.as_deref().map(parse_date).transpose()?;
            let date_start = p.date_start.as_deref().map(parse_date).transpose()?;
            let date_end = p.date_end.as_deref().map(parse_date).transpose()?;

            let (filter_start, filter_end) = resolve_date_filter(
                date,
                date_start,
                date_end,
                p.today.unwrap_or(false),
                p.this_week.unwrap_or(false),
                p.this_month.unwrap_or(false),
                week_start,
            );

            let paths = collect_entries(self.journal_dir.as_deref())?;
            let has_filter = filter_start.is_some() || filter_end.is_some();
            let mut records: Vec<serde_json::Value> = Vec::new();

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

                let match_labels: Vec<&str> = if has_filter {
                    let labels = compute_labels(&entry, filter_start, filter_end);
                    if labels.is_empty() {
                        continue;
                    }
                    labels.into_iter().map(|l| l.as_str()).collect()
                } else {
                    vec![]
                };

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
                    v["match_labels"] = serde_json::json!(match_labels);
                }
                records.push(v);
            }

            Ok(serde_json::to_string_pretty(&records)?)
        })()
        .map_err(|e| e.to_string())
    }

    #[tool(description = "Show the contents of a journal entry by ID prefix or file path")]
    fn entry_show(&self, Parameters(p): Parameters<EntryShowParams>) -> Result<String, String> {
        (|| -> anyhow::Result<String> {
            let path = self.resolve_entry(&p.entry)?;
            let entry = read_entry(&path)?;
            let fm = &entry.frontmatter;

            let mut out = format!("# {}\n", entry.title());

            if let Some(ts) = fm.created_at {
                out.push_str(&format!("created:  {}\n", ts.format("%Y-%m-%dT%H:%M")));
            }
            if let Some(ts) = fm.updated_at {
                out.push_str(&format!("updated:  {}\n", ts.format("%Y-%m-%dT%H:%M")));
            }
            if !fm.tags.is_empty() {
                out.push_str(&format!("tags:     {}\n", fm.tags.join(", ")));
            }
            if let Some(task) = &fm.task {
                let status = task.status.as_deref().unwrap_or("open");
                match task.due {
                    Some(d) => out.push_str(&format!(
                        "task:     {status} (due {})\n",
                        d.format("%Y-%m-%d")
                    )),
                    None => out.push_str(&format!("task:     {status}\n")),
                }
                if let Some(ca) = task.closed_at {
                    out.push_str(&format!("closed:   {}\n", ca.format("%Y-%m-%dT%H:%M")));
                }
            }
            if let Some(event) = &fm.event {
                match (event.start, event.end) {
                    (Some(s), Some(e)) => out.push_str(&format!(
                        "event:    {} – {}\n",
                        s.format("%Y-%m-%d"),
                        e.format("%Y-%m-%d")
                    )),
                    (Some(s), None) => {
                        out.push_str(&format!("event:    from {}\n", s.format("%Y-%m-%d")))
                    }
                    (None, Some(e)) => {
                        out.push_str(&format!("event:    until {}\n", e.format("%Y-%m-%d")))
                    }
                    (None, None) => out.push_str("event:    (no dates)\n"),
                }
            }

            out.push('\n');
            out.push_str(&entry.body);
            Ok(out)
        })()
        .map_err(|e| e.to_string())
    }

    #[tool(description = "Create a new journal entry")]
    fn entry_new(&self, Parameters(p): Parameters<EntryNewParams>) -> Result<String, String> {
        (|| -> anyhow::Result<String> {
            let journal = self.open_journal()?;
            let dest = journal.root.join(new_entry_path(&p.name));

            if dest.exists() {
                anyhow::bail!("{} already exists", dest.display());
            }

            let tags = p
                .tags
                .as_deref()
                .map(|s| {
                    s.split(',')
                        .map(|t| t.trim().to_owned())
                        .filter(|t| !t.is_empty())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();

            let task = if p.task_due.is_some()
                || p.task_status.is_some()
                || p.task_closed_at.is_some()
            {
                let due = p.task_due.as_deref().map(parse_datetime_end).transpose()?;
                let status = p.task_status.clone();
                let inactive =
                    matches!(status.as_deref(), Some("done" | "cancelled" | "archived"));
                let closed_at = p
                    .task_closed_at
                    .as_deref()
                    .map(parse_datetime)
                    .transpose()?
                    .or_else(|| inactive.then(|| chrono::Local::now().naive_local()));
                Some(TaskMeta { due, status, closed_at })
            } else {
                None
            };

            let event = if p.event_start.is_some() || p.event_end.is_some() {
                let start = p.event_start.as_deref().map(parse_datetime).transpose()?;
                let end = p.event_end.as_deref().map(parse_datetime_end).transpose()?;
                Some(EventMeta { start, end })
            } else {
                None
            };

            let now = chrono::Local::now().naive_local();
            let entry = Entry {
                path: dest.clone(),
                frontmatter: Frontmatter {
                    title: p.title.or_else(|| Some(p.name.clone())),
                    slug: p.slug,
                    tags,
                    created_at: Some(now),
                    updated_at: Some(now),
                    task,
                    event,
                },
                body: p.body,
            };

            std::fs::create_dir_all(dest.parent().unwrap())
                .with_context(|| format!("failed to create directory for {}", dest.display()))?;
            std::fs::write(&dest, render_entry(&entry))
                .with_context(|| format!("failed to write {}", dest.display()))?;

            Ok(format!("created: {}", dest.display()))
        })()
        .map_err(|e| e.to_string())
    }

    #[tool(description = "Update frontmatter fields of an existing journal entry")]
    fn entry_set(&self, Parameters(p): Parameters<EntrySetParams>) -> Result<String, String> {
        (|| -> anyhow::Result<String> {
            if p.title.is_none()
                && p.slug.is_none()
                && p.tags.is_none()
                && p.task_due.is_none()
                && p.task_status.is_none()
                && p.task_closed_at.is_none()
                && p.event_start.is_none()
                && p.event_end.is_none()
            {
                anyhow::bail!("nothing to update — specify at least one field");
            }

            let path = self.resolve_entry(&p.entry)?;
            let mut entry = read_entry(&path)?;

            if let Some(t) = p.title {
                entry.frontmatter.title = Some(t);
            }
            if let Some(s) = p.slug {
                entry.frontmatter.slug = Some(s);
            }
            if let Some(tags_str) = p.tags {
                entry.frontmatter.tags = tags_str
                    .split(',')
                    .map(|t| t.trim().to_owned())
                    .filter(|t| !t.is_empty())
                    .collect();
            }

            if p.task_due.is_some() || p.task_status.is_some() || p.task_closed_at.is_some() {
                let task = entry.frontmatter.task.get_or_insert_with(Default::default);
                if let Some(d) = p.task_due.as_deref() {
                    task.due = Some(parse_datetime_end(d)?);
                }
                if let Some(s) = p.task_status {
                    let inactive = matches!(s.as_str(), "done" | "cancelled" | "archived");
                    task.status = Some(s);
                    if inactive && task.closed_at.is_none() && p.task_closed_at.is_none() {
                        task.closed_at = Some(chrono::Local::now().naive_local());
                    }
                }
                if let Some(ca) = p.task_closed_at.as_deref() {
                    task.closed_at = Some(parse_datetime(ca)?);
                }
            }

            if p.event_start.is_some() || p.event_end.is_some() {
                let event = entry.frontmatter.event.get_or_insert_with(Default::default);
                if let Some(s) = p.event_start.as_deref() {
                    event.start = Some(parse_datetime(s)?);
                }
                if let Some(e) = p.event_end.as_deref() {
                    event.end = Some(parse_datetime_end(e)?);
                }
            }

            write_entry(&mut entry)?;
            Ok(format!("updated: {}", path.display()))
        })()
        .map_err(|e| e.to_string())
    }
}

#[tool_handler]
impl ServerHandler for ArchelonServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions(
                "Archelon is a Markdown-based journal/task manager. \
                 Use entry_list to browse entries, entry_show to read one, \
                 entry_new to create, and entry_set to update metadata."
                    .to_owned(),
            )
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn parse_date(s: &str) -> anyhow::Result<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .with_context(|| format!("`{s}` is not a valid date — expected YYYY-MM-DD"))
}

fn parse_datetime(s: &str) -> anyhow::Result<NaiveDateTime> {
    if let Ok(dt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S") {
        return Ok(dt);
    }
    if let Ok(dt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M") {
        return Ok(dt);
    }
    if let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return Ok(d.and_hms_opt(0, 0, 0).unwrap());
    }
    anyhow::bail!("`{s}` is not a valid date/datetime — expected YYYY-MM-DD or YYYY-MM-DDTHH:MM")
}

fn parse_datetime_end(s: &str) -> anyhow::Result<NaiveDateTime> {
    if let Ok(dt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S") {
        return Ok(dt);
    }
    if let Ok(dt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M") {
        return Ok(dt);
    }
    if let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return Ok(d.and_hms_opt(23, 59, 59).unwrap());
    }
    anyhow::bail!("`{s}` is not a valid date/datetime — expected YYYY-MM-DD or YYYY-MM-DDTHH:MM")
}

fn resolve_date_filter(
    date: Option<NaiveDate>,
    date_start: Option<NaiveDate>,
    date_end: Option<NaiveDate>,
    today: bool,
    this_week: bool,
    this_month: bool,
    week_start: WeekStart,
) -> (Option<NaiveDate>, Option<NaiveDate>) {
    if let Some(d) = date {
        return (Some(d), Some(d));
    }
    if today {
        let d = chrono::Local::now().date_naive();
        return (Some(d), Some(d));
    }
    if this_week {
        let today = chrono::Local::now().date_naive();
        let days_back = match week_start {
            WeekStart::Monday => today.weekday().num_days_from_monday(),
            WeekStart::Sunday => today.weekday().num_days_from_sunday(),
        };
        let start = today - Duration::days(days_back as i64);
        let end = start + Duration::days(6);
        return (Some(start), Some(end));
    }
    if this_month {
        let today = chrono::Local::now().date_naive();
        let start = today.with_day(1).unwrap();
        let end = NaiveDate::from_ymd_opt(
            if today.month() == 12 { today.year() + 1 } else { today.year() },
            if today.month() == 12 { 1 } else { today.month() + 1 },
            1,
        )
        .unwrap()
            - Duration::days(1);
        return (Some(start), Some(end));
    }
    (date_start, date_end)
}

fn collect_entries(journal_dir: Option<&Path>) -> anyhow::Result<Vec<PathBuf>> {
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

#[derive(Debug, Clone, Copy)]
enum MatchLabel {
    Todo,
    Closed,
    Event,
    Created,
    Updated,
}

impl MatchLabel {
    fn as_str(self) -> &'static str {
        match self {
            MatchLabel::Todo => "TODO",
            MatchLabel::Closed => "CLOSED",
            MatchLabel::Event => "EVENT",
            MatchLabel::Created => "CREATED",
            MatchLabel::Updated => "UPDATED",
        }
    }
}

fn compute_labels(
    entry: &Entry,
    start: Option<NaiveDate>,
    end: Option<NaiveDate>,
) -> Vec<MatchLabel> {
    let mut labels = Vec::new();

    if let Some(task) = &entry.frontmatter.task {
        let inactive = matches!(
            task.status.as_deref().unwrap_or("open"),
            "done" | "cancelled" | "archived"
        );
        if !inactive {
            labels.push(MatchLabel::Todo);
        } else {
            if task.due.is_some_and(|d| date_in_range(d.date(), start, end)) {
                labels.push(MatchLabel::Todo);
            }
            if task.closed_at.is_some_and(|c| date_in_range(c.date(), start, end)) {
                labels.push(MatchLabel::Closed);
            }
        }
    }

    if let Some(event) = &entry.frontmatter.event {
        let event_start = event.start.map(|s| s.date());
        let event_end = event.end.map(|e| e.date());
        let overlaps_end = end.map_or(true, |re| event_start.map_or(true, |es| es <= re));
        let overlaps_start = start.map_or(true, |rs| event_end.map_or(true, |ee| ee >= rs));
        if overlaps_end && overlaps_start {
            labels.push(MatchLabel::Event);
        }
    }

    if entry.frontmatter.created_at.is_some_and(|c| date_in_range(c.date(), start, end)) {
        labels.push(MatchLabel::Created);
    }
    if entry.frontmatter.updated_at.is_some_and(|u| date_in_range(u.date(), start, end)) {
        labels.push(MatchLabel::Updated);
    }

    labels
}

fn date_in_range(date: NaiveDate, start: Option<NaiveDate>, end: Option<NaiveDate>) -> bool {
    start.map_or(true, |s| date >= s) && end.map_or(true, |e| date <= e)
}

fn detect_timezone() -> String {
    if let Ok(tz) = std::env::var("TZ") {
        if !tz.is_empty() {
            return tz;
        }
    }
    if let Ok(contents) = std::fs::read_to_string("/etc/timezone") {
        let tz = contents.trim().to_owned();
        if !tz.is_empty() {
            return tz;
        }
    }
    "UTC".to_owned()
}

// ── main ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Log to stderr so stdout remains clean for the MCP JSON-RPC protocol
    tracing_subscriber::fmt().with_writer(std::io::stderr).init();

    let journal_dir = std::env::var("ARCHELON_JOURNAL_DIR").ok().map(PathBuf::from);
    let server = ArchelonServer::new(journal_dir);
    let service = server.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
