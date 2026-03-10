use std::path::{Path, PathBuf};

use anyhow::Context as _;
use archelon_core::{
    cache,
    entry_ref::EntryRef,
    journal::{Journal, WeekStart},
    ops::{self, EntryFields, EntryFilter, EntryTreeNode, FieldSelector, SortField, SortOrder},
    parser::read_entry,
    period::{parse_datetime, parse_datetime_end, parse_period},
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
        ops::resolve_entry(&EntryRef::parse(entry), self.journal_dir.as_deref())
            .map_err(Into::into)
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
    /// Period to match against timestamp fields.
    /// When no field selectors are set, the period applies to all fields (OR).
    ///
    /// Accepted formats: today | this_week | this_month | none |
    /// YYYY-MM-DD | YYYY-MM-DD,YYYY-MM-DD | YYYY-MM-DDTHH:MM,YYYY-MM-DDTHH:MM
    period: Option<String>,

    /// Restrict period matching to task due date.
    /// Without period: include entries that have a task_due set.
    task_due: Option<bool>,

    /// Restrict period matching to event span (overlap semantics).
    /// Without period: include entries that have an event set.
    event_span: Option<bool>,

    /// Restrict period matching to created_at timestamp.
    created_at: Option<bool>,

    /// Restrict period matching to updated_at timestamp.
    updated_at: Option<bool>,

    /// AND filter: include only entries whose task status matches one of these values.
    /// Provide as an array, e.g. ["open", "in_progress"]
    task_status: Option<Vec<String>>,

    /// AND filter: include only entries that have ALL of these tags.
    /// Provide as an array, e.g. ["work", "urgent"]
    tags: Option<Vec<String>>,

    /// OR filter with period: include tasks whose due date is in the past and closed_at is absent.
    /// Can be combined with period; either condition is sufficient for inclusion.
    overdue: Option<bool>,

    /// OR filter with period: include tasks that have started_at set, closed_at absent,
    /// and (when period is given) started_at ≤ period end.
    task_started: Option<bool>,

    /// Field to sort results by.
    /// Accepted values: id | title | task_status | created_at | updated_at | task_due | event_start | event_end
    sort_by: Option<String>,

    /// Sort direction: "asc" (default) or "desc"
    sort_order: Option<String>,
}

/// Same filter parameters as [`EntryListParams`] but for the tree tool.
type EntryTreeParams = EntryListParams;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct EntryShowParams {
    /// File path to the entry, or an ID / ID prefix
    entry: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct EntryNewParams {
    /// Title of the entry — written into the frontmatter and used to generate the filename slug
    title: String,
    /// Body content (Markdown)
    body: String,
    /// Slug override in the frontmatter
    slug: Option<String>,
    /// Tags as comma-separated string (e.g. "work,project")
    tags: Option<String>,
    /// Task due date/time (YYYY-MM-DD or YYYY-MM-DDTHH:MM)
    task_due: Option<String>,
    /// Task status (open | in_progress | done | cancelled | archived)
    task_status: Option<String>,
    /// Task start date/time; set automatically when status → in_progress
    task_started_at: Option<String>,
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
    /// New body content (Markdown). Replaces the existing body.
    body: Option<String>,
    /// New slug override
    slug: Option<String>,
    /// New tags as comma-separated string. Pass empty string to clear all tags.
    tags: Option<String>,
    /// Task due date/time (YYYY-MM-DD or YYYY-MM-DDTHH:MM)
    task_due: Option<String>,
    /// Task status (open | in_progress | done | cancelled | archived)
    task_status: Option<String>,
    /// Task start date/time; set automatically when status → in_progress
    task_started_at: Option<String>,
    /// Task close date/time
    task_closed_at: Option<String>,
    /// Event start date/time (YYYY-MM-DD or YYYY-MM-DDTHH:MM)
    event_start: Option<String>,
    /// Event end date/time (YYYY-MM-DD or YYYY-MM-DDTHH:MM)
    event_end: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct EntryCheckParams {
    /// File path to the entry, or an ID / ID prefix
    entry: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct EntryFixParams {
    /// File path to the entry, or an ID / ID prefix
    entry: String,
    /// If true, also update updated_at to the current time
    #[serde(default)]
    touch: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct EntryRemoveParams {
    /// File path to the entry, or an ID / ID prefix
    entry: String,
}

// ── helpers for parameter parsing ─────────────────────────────────────────────

fn parse_entry_fields(
    slug: Option<String>,
    tags: Option<String>,
    task_due: Option<&str>,
    task_status: Option<String>,
    task_started_at: Option<&str>,
    task_closed_at: Option<&str>,
    event_start: Option<&str>,
    event_end: Option<&str>,
) -> anyhow::Result<EntryFields> {
    Ok(EntryFields {
        slug,
        tags: tags.as_deref().map(|s| {
            s.split(',')
                .map(|t| t.trim().to_owned())
                .filter(|t| !t.is_empty())
                .collect()
        }),
        task_due: task_due
            .map(|s| parse_datetime_end(s).map_err(anyhow::Error::msg))
            .transpose()?,
        task_status,
        task_started_at: task_started_at
            .map(|s| parse_datetime(s).map_err(anyhow::Error::msg))
            .transpose()?,
        task_closed_at: task_closed_at
            .map(|s| parse_datetime(s).map_err(anyhow::Error::msg))
            .transpose()?,
        event_start: event_start
            .map(|s| parse_datetime(s).map_err(anyhow::Error::msg))
            .transpose()?,
        event_end: event_end
            .map(|s| parse_datetime_end(s).map_err(anyhow::Error::msg))
            .transpose()?,
    })
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
        Use `period` to specify a time range. \
        Use field selectors (task_due, event_span, created_at, updated_at) to restrict which fields \
        the period applies to; omitting all selectors applies the period to all fields (OR). \
        Without a period, field selectors filter entries where that field is present. \
        event_span uses interval-overlap semantics so in-progress events are included. \
        task_status, tags, and overdue are independent AND/OR filters.")]
    fn entry_list(&self, Parameters(p): Parameters<EntryListParams>) -> Result<String, String> {
        (|| -> anyhow::Result<String> {
            let week_start = self.week_start();
            let parse = |s: &str| parse_period(s, week_start).map_err(anyhow::Error::msg);

            let filter = EntryFilter {
                period: p.period.as_deref().map(parse).transpose()?,
                fields: FieldSelector {
                    task_due:   p.task_due.unwrap_or(false),
                    event_span: p.event_span.unwrap_or(false),
                    created_at: p.created_at.unwrap_or(false),
                    updated_at: p.updated_at.unwrap_or(false),
                },
                task_status: p.task_status.unwrap_or_default(),
                tags: p.tags.unwrap_or_default(),
                overdue: p.overdue.unwrap_or(false),
                task_started: p.task_started.unwrap_or(false),
                sort_by: p.sort_by.as_deref()
                    .map(|s| s.parse::<SortField>().map_err(anyhow::Error::msg))
                    .transpose()?,
                sort_order: p.sort_order.as_deref()
                    .map(|s| s.parse::<SortOrder>().map_err(anyhow::Error::msg))
                    .transpose()?
                    .unwrap_or_default(),
            };

            let has_filter = filter.has_any_filter();
            let entries = ops::list_entries(self.journal_dir.as_deref(), &filter)?;

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

            Ok(serde_json::to_string_pretty(&records)?)
        })()
        .map_err(|e| e.to_string())
    }

    #[tool(description = "List journal entries as a JSON tree, preserving parent–child relationships. \
        Accepts the same filter parameters as entry_list. \
        Each node contains an `id`, `title`, `task`, `tags`, and a `children` array of nested nodes.")]
    fn entry_tree(&self, Parameters(p): Parameters<EntryTreeParams>) -> Result<String, String> {
        (|| -> anyhow::Result<String> {
            let week_start = self.week_start();
            let parse = |s: &str| parse_period(s, week_start).map_err(anyhow::Error::msg);

            let filter = EntryFilter {
                period: p.period.as_deref().map(parse).transpose()?,
                fields: FieldSelector {
                    task_due:   p.task_due.unwrap_or(false),
                    event_span: p.event_span.unwrap_or(false),
                    created_at: p.created_at.unwrap_or(false),
                    updated_at: p.updated_at.unwrap_or(false),
                },
                task_status: p.task_status.unwrap_or_default(),
                tags: p.tags.unwrap_or_default(),
                overdue: p.overdue.unwrap_or(false),
                task_started: p.task_started.unwrap_or(false),
                sort_by: p.sort_by.as_deref()
                    .map(|s| s.parse::<SortField>().map_err(anyhow::Error::msg))
                    .transpose()?,
                sort_order: p.sort_order.as_deref()
                    .map(|s| s.parse::<SortOrder>().map_err(anyhow::Error::msg))
                    .transpose()?
                    .unwrap_or_default(),
            };

            let has_filter = filter.has_any_filter();
            let entries = ops::list_entries(self.journal_dir.as_deref(), &filter)?;
            let roots = ops::build_entry_tree(entries);

            fn node_to_json(node: &EntryTreeNode, has_filter: bool) -> serde_json::Value {
                let entry = &node.entry;
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
                    "children": node.children.iter().map(|c| node_to_json(c, has_filter)).collect::<Vec<_>>(),
                });
                if has_filter {
                    v["match_labels"] = serde_json::json!(
                        node.labels.iter().map(|l| l.as_str()).collect::<Vec<_>>()
                    );
                }
                v
            }

            let records: Vec<_> = roots.iter().map(|n| node_to_json(n, has_filter)).collect();
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
            out.push_str(&format!("created:  {}\n", fm.created_at.format("%Y-%m-%dT%H:%M")));
            out.push_str(&format!("updated:  {}\n", fm.updated_at.format("%Y-%m-%dT%H:%M")));
            if !fm.tags.is_empty() {
                out.push_str(&format!("tags:     {}\n", fm.tags.join(", ")));
            }
            if let Some(task) = &fm.task {
                let status = task.status.as_str();
                match task.due {
                    Some(d) => out.push_str(&format!("task:     {status} (due {})\n", d.format("%Y-%m-%d"))),
                    None    => out.push_str(&format!("task:     {status}\n")),
                }
                if let Some(ca) = task.closed_at {
                    out.push_str(&format!("closed:   {}\n", ca.format("%Y-%m-%dT%H:%M")));
                }
            }
            if let Some(event) = &fm.event {
                out.push_str(&format!("event:    {} – {}\n", event.start.format("%Y-%m-%d"), event.end.format("%Y-%m-%d")));
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
            let fields = parse_entry_fields(
                p.slug,
                p.tags,
                p.task_due.as_deref(),
                p.task_status,
                p.task_started_at.as_deref(),
                p.task_closed_at.as_deref(),
                p.event_start.as_deref(),
                p.event_end.as_deref(),
            )?;
            let dest = ops::create_entry(&journal, &p.title, p.body, fields)?;
            Ok(format!("created: {}", dest.display()))
        })()
        .map_err(|e| e.to_string())
    }

    #[tool(description = "Update frontmatter fields of an existing journal entry")]
    fn entry_set(&self, Parameters(p): Parameters<EntrySetParams>) -> Result<String, String> {
        (|| -> anyhow::Result<String> {
            if p.title.is_none()
                && p.body.is_none()
                && p.slug.is_none()
                && p.tags.is_none()
                && p.task_due.is_none()
                && p.task_status.is_none()
                && p.task_started_at.is_none()
                && p.task_closed_at.is_none()
                && p.event_start.is_none()
                && p.event_end.is_none()
            {
                anyhow::bail!("nothing to update — specify at least one field");
            }

            let path = self.resolve_entry(&p.entry)?;
            let fields = parse_entry_fields(
                p.slug,
                p.tags,
                p.task_due.as_deref(),
                p.task_status,
                p.task_started_at.as_deref(),
                p.task_closed_at.as_deref(),
                p.event_start.as_deref(),
                p.event_end.as_deref(),
            )?;
            let msg = if let Some(new_path) = ops::update_entry(&path, p.title, p.body, fields)? {
                format!("updated and renamed: {}", new_path.display())
            } else {
                format!("updated: {}", path.display())
            };
            Ok(msg)
        })()
        .map_err(|e| e.to_string())
    }

    #[tool(description = "Check whether an entry's frontmatter and filename are valid. \
        Returns 'ok' or a list of issues (e.g. filename mismatch).")]
    fn entry_check(&self, Parameters(p): Parameters<EntryCheckParams>) -> Result<String, String> {
        (|| -> anyhow::Result<String> {
            let path = self.resolve_entry(&p.entry)?;
            let issues = ops::check_entry(&path)?;
            if issues.is_empty() {
                Ok(format!("ok: {}", path.display()))
            } else {
                let lines: Vec<String> = issues
                    .iter()
                    .map(|i| format!("{}: {}", path.display(), i.as_str()))
                    .collect();
                Ok(lines.join("\n"))
            }
        })()
        .map_err(|e| e.to_string())
    }

    #[tool(description = "Normalize an entry: sync closed_at, rename the file to match its frontmatter \
        ID and title/slug, and optionally refresh updated_at (touch=true). \
        Reports the rename or confirms the filename is already correct.")]
    fn entry_fix(&self, Parameters(p): Parameters<EntryFixParams>) -> Result<String, String> {
        (|| -> anyhow::Result<String> {
            let path = self.resolve_entry(&p.entry)?;
            match ops::fix_entry(&path, p.touch)? {
                Some(new_path) => Ok(format!(
                    "renamed: {} → {}",
                    path.file_name().unwrap_or_default().to_string_lossy(),
                    new_path.file_name().unwrap_or_default().to_string_lossy(),
                )),
                None => Ok(format!("ok: {} (already correct)", path.display())),
            }
        })()
        .map_err(|e| e.to_string())
    }

    #[tool(description = "Delete an entry file from the journal")]
    fn entry_remove(&self, Parameters(p): Parameters<EntryRemoveParams>) -> Result<String, String> {
        (|| -> anyhow::Result<String> {
            let path = self.resolve_entry(&p.entry)?;
            ops::remove_entry(&path)?;
            // Keep the cache consistent after explicit deletion.
            if let Ok(journal) = self.open_journal() {
                if let Ok(conn) = cache::open_cache(&journal) {
                    let _ = cache::remove_from_cache(&conn, &path);
                }
            }
            Ok(format!("removed: {}", path.display()))
        })()
        .map_err(|e| e.to_string())
    }

    #[tool(description = "Show cache location, schema version, and entry/tag counts.")]
    fn cache_info(&self, _: Parameters<serde_json::Value>) -> Result<String, String> {
        (|| -> anyhow::Result<String> {
            let journal = self.open_journal()?;
            let conn = cache::open_cache(&journal)?;
            let info = cache::cache_info(&journal, &conn)?;
            Ok(format!(
                "path: {}\nschema version: v{} (app: v{})\nfiles tracked: {}\nentries: {}\nunique tags: {}",
                info.db_path.display(),
                info.schema_version,
                cache::SCHEMA_VERSION,
                info.file_count,
                info.entry_count,
                info.unique_tag_count,
            ))
        })()
        .map_err(|e| e.to_string())
    }

    #[tool(description = "Incrementally sync the cache with the current journal state. \
        Re-indexes files whose mtime has changed and removes entries for deleted files. \
        Returns the number of entries in the cache after sync.")]
    fn cache_sync(&self, _: Parameters<serde_json::Value>) -> Result<String, String> {
        (|| -> anyhow::Result<String> {
            let journal = self.open_journal()?;
            let conn = cache::open_cache(&journal)?;
            cache::sync_cache(&journal, &conn)?;
            let info = cache::cache_info(&journal, &conn)?;
            Ok(format!("synced: {} entries", info.entry_count))
        })()
        .map_err(|e| e.to_string())
    }

    #[tool(description = "Rebuild the local SQLite cache from scratch. \
        Use this after updating archelon when the schema has changed, \
        or when the cache has become inconsistent. \
        Returns the number of entries indexed.")]
    fn cache_rebuild(&self, _: Parameters<serde_json::Value>) -> Result<String, String> {
        (|| -> anyhow::Result<String> {
            let journal = self.open_journal()?;
            let conn = cache::rebuild_cache(&journal)?;
            cache::sync_cache(&journal, &conn)?;
            let info = cache::cache_info(&journal, &conn)?;
            Ok(format!("rebuilt: {} entries indexed", info.entry_count))
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
pub async fn run(journal_dir: Option<&Path>, ) -> anyhow::Result<()> {

    // Log to stderr so stdout remains clean for the MCP JSON-RPC protocol
    tracing_subscriber::fmt().with_writer(std::io::stderr).init();

    let server = ArchelonServer::new(journal_dir.map(|x| x.to_path_buf()));
    let service = server.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
