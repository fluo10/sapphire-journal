use std::path::{Path, PathBuf};

use anyhow::Context as _;
use archelon_core::{
    entry::{Entry, EventMeta, Frontmatter, TaskMeta},
    journal::{is_managed_filename, Journal, WeekStart, new_entry_path},
    parser::{read_entry, render_entry, write_entry},
    period::{parse_datetime, parse_datetime_end, parse_period, Period},
};
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

    fn week_start(&self) -> WeekStart {
        self.open_journal()
            .and_then(|j| j.config().map_err(Into::into))
            .map(|c| c.journal.week_start)
            .unwrap_or_default()
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
    /// Convenience filter applied to all timestamp fields simultaneously (OR across fields).
    /// Equivalent to setting task_due, event_start, event_end, created_at, updated_at to the same period.
    ///
    /// Accepted formats: today | this_week | this_month | none |
    /// YYYY-MM-DD | YYYY-MM-DD,YYYY-MM-DD | YYYY-MM-DDTHH:MM,YYYY-MM-DDTHH:MM
    period: Option<String>,

    /// Filter by task due date (same PERIOD format as `period`)
    task_due: Option<String>,

    /// Filter by event start date (same PERIOD format as `period`)
    event_start: Option<String>,

    /// Filter by event end date (same PERIOD format as `period`)
    event_end: Option<String>,

    /// Filter by created_at timestamp (same PERIOD format as `period`)
    created_at: Option<String>,

    /// Filter by updated_at timestamp (same PERIOD format as `period`)
    updated_at: Option<String>,

    /// AND filter: include only entries whose task status matches one of these values.
    /// Provide as an array, e.g. ["open", "in_progress"]
    task_status: Option<Vec<String>>,
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

// ── filter ────────────────────────────────────────────────────────────────────

struct EntryFilter {
    period: Option<Period>,
    task_due: Option<Period>,
    event_start: Option<Period>,
    event_end: Option<Period>,
    created_at: Option<Period>,
    updated_at: Option<Period>,
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

    fn matches(&self, entry: &Entry) -> (bool, Vec<&'static str>) {
        let mut labels: Vec<&'static str> = Vec::new();

        let timestamp_ok = if self.has_timestamp_filter() {
            let task_due_val = entry.frontmatter.task.as_ref().and_then(|t| t.due);
            let event_start_val = entry.frontmatter.event.as_ref().and_then(|e| e.start);
            let event_end_val = entry.frontmatter.event.as_ref().and_then(|e| e.end);
            let created_val = entry.frontmatter.created_at;
            let updated_val = entry.frontmatter.updated_at;

            macro_rules! check {
                ($filter:expr, $val:expr, $label:literal) => {
                    if let Some(p) = &$filter {
                        if p.matches($val) {
                            labels.push($label);
                        }
                    }
                };
            }

            if let Some(p) = &self.period {
                if p.matches(task_due_val) { labels.push("TASK_DUE"); }
                if p.matches(event_start_val) { labels.push("EVENT_START"); }
                if p.matches(event_end_val) { labels.push("EVENT_END"); }
                if p.matches(created_val) { labels.push("CREATED"); }
                if p.matches(updated_val) { labels.push("UPDATED"); }
            }

            check!(self.task_due, task_due_val, "TASK_DUE");
            check!(self.event_start, event_start_val, "EVENT_START");
            check!(self.event_end, event_end_val, "EVENT_END");
            check!(self.created_at, created_val, "CREATED");
            check!(self.updated_at, updated_val, "UPDATED");

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

    #[tool(description = "List journal entries as JSON. \
        Timestamp filters (period, task_due, event_start, event_end, created_at, updated_at) are ORed: \
        an entry matches if any specified timestamp field falls within the given period. \
        task_status is ANDed on top: if provided, the entry must also have a matching task status.")]
    fn entry_list(&self, Parameters(p): Parameters<EntryListParams>) -> Result<String, String> {
        (|| -> anyhow::Result<String> {
            let week_start = self.week_start();
            let parse = |s: &str| {
                parse_period(s, week_start).map_err(anyhow::Error::msg)
            };

            let filter = EntryFilter {
                period: p.period.as_deref().map(parse).transpose()?,
                task_due: p.task_due.as_deref().map(parse).transpose()?,
                event_start: p.event_start.as_deref().map(parse).transpose()?,
                event_end: p.event_end.as_deref().map(parse).transpose()?,
                created_at: p.created_at.as_deref().map(parse).transpose()?,
                updated_at: p.updated_at.as_deref().map(parse).transpose()?,
                task_status: p.task_status.unwrap_or_default(),
            };

            let has_filter = filter.has_timestamp_filter() || !filter.task_status.is_empty();
            let paths = collect_entries(self.journal_dir.as_deref())?;
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

                let (include, labels) = filter.matches(&entry);
                if has_filter && !include {
                    continue;
                }

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
                    v["match_labels"] = serde_json::json!(labels);
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
                let due = p.task_due.as_deref()
                    .map(|s| parse_datetime_end(s).map_err(anyhow::Error::msg))
                    .transpose()?;
                let status = p.task_status.clone();
                let inactive =
                    matches!(status.as_deref(), Some("done" | "cancelled" | "archived"));
                let closed_at = p
                    .task_closed_at
                    .as_deref()
                    .map(|s| parse_datetime(s).map_err(anyhow::Error::msg))
                    .transpose()?
                    .or_else(|| inactive.then(|| chrono::Local::now().naive_local()));
                Some(TaskMeta { due, status, closed_at })
            } else {
                None
            };

            let event = if p.event_start.is_some() || p.event_end.is_some() {
                let start = p.event_start.as_deref()
                    .map(|s| parse_datetime(s).map_err(anyhow::Error::msg))
                    .transpose()?;
                let end = p.event_end.as_deref()
                    .map(|s| parse_datetime_end(s).map_err(anyhow::Error::msg))
                    .transpose()?;
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
                    task.due = Some(parse_datetime_end(d).map_err(anyhow::Error::msg)?);
                }
                if let Some(s) = p.task_status {
                    let inactive = matches!(s.as_str(), "done" | "cancelled" | "archived");
                    task.status = Some(s);
                    if inactive && task.closed_at.is_none() && p.task_closed_at.is_none() {
                        task.closed_at = Some(chrono::Local::now().naive_local());
                    }
                }
                if let Some(ca) = p.task_closed_at.as_deref() {
                    task.closed_at = Some(parse_datetime(ca).map_err(anyhow::Error::msg)?);
                }
            }

            if p.event_start.is_some() || p.event_end.is_some() {
                let event = entry.frontmatter.event.get_or_insert_with(Default::default);
                if let Some(s) = p.event_start.as_deref() {
                    event.start = Some(parse_datetime(s).map_err(anyhow::Error::msg)?);
                }
                if let Some(e) = p.event_end.as_deref() {
                    event.end = Some(parse_datetime_end(e).map_err(anyhow::Error::msg)?);
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
