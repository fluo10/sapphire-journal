use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::Context as _;
use archelon_core::{
    cache,
    entry_ref::EntryRef,
    journal::{Journal, WeekStart},
    ops::{self, EntryFields, EntryFilter, EntryListItem, FieldSelector, SortField, SortOrder, UpdateOption},
    parser::read_entry,
    period::{parse_datetime, parse_datetime_end, parse_period},
    user_config::UserConfig,
    JournalState,
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

#[derive(Clone)]
struct ArchelonServer {
    /// Default journal directory passed at server startup (if any).
    default_dir: Option<PathBuf>,
    /// Cached journal + SQLite connection, shared across tool calls.
    state: Arc<Mutex<Option<JournalState>>>,
    tool_router: ToolRouter<Self>,
}

impl std::fmt::Debug for ArchelonServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ArchelonServer")
            .field("default_dir", &self.default_dir)
            .finish_non_exhaustive()
    }
}

impl ArchelonServer {
    fn new(journal_dir: Option<PathBuf>) -> Self {
        Self {
            default_dir: journal_dir,
            state: Arc::new(Mutex::new(None)),
            tool_router: Self::tool_router(),
        }
    }

    /// Provide access to the cached state, auto-opening on first use.
    fn with_state<F, T>(&self, f: F) -> anyhow::Result<T>
    where
        F: FnOnce(&JournalState) -> anyhow::Result<T>,
    {
        let mut guard = self.state.lock().unwrap();
        if guard.is_none() {
            let journal = self.open_default_journal()?;
            let state = JournalState::open(journal)?;
            let user_cfg = UserConfig::load().unwrap_or_default();
            let _ = state.load_vector_store(&user_cfg); // best-effort; errors are non-fatal
            let _ = state.load_embedder(&user_cfg);     // best-effort; errors are non-fatal
            *guard = Some(state);
        }
        f(guard.as_ref().unwrap())
    }

    /// Open a journal and replace the cached state.
    /// Returns (journal_root_string, entry_count).
    fn reload_state(&self, journal_dir: Option<&Path>) -> anyhow::Result<(String, u64)> {
        let journal = match journal_dir {
            Some(dir) => Journal::from_root(dir.to_path_buf())
                .context("not an archelon journal — run `journal_init` first")?,
            None => self.open_default_journal()?,
        };
        let state = JournalState::open(journal)?;
        state.sync()?;
        let user_cfg = UserConfig::load().unwrap_or_default();
        let _ = state.load_vector_store(&user_cfg); // best-effort; errors are non-fatal
        let _ = state.load_embedder(&user_cfg);     // best-effort; errors are non-fatal
        let info = state.cache_info()?;
        let root = state.journal.root.display().to_string();
        let count = info.entry_count;
        *self.state.lock().unwrap() = Some(state);
        Ok((root, count))
    }

    fn open_default_journal(&self) -> anyhow::Result<Journal> {
        match &self.default_dir {
            Some(dir) => Journal::from_root(dir.clone())
                .context("not an archelon journal — run `journal_init` first"),
            None => Journal::find()
                .context("not in an archelon journal — run `journal_init` first"),
        }
    }

    fn week_start(&self) -> WeekStart {
        self.with_state(|s| s.journal.config().map(|c| c.journal.week_start).map_err(Into::into))
            .unwrap_or_default()
    }
}

// ── parameter structs ─────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct JournalOpenParams {
    /// Path to the journal root directory.
    /// If omitted, searches upward from the current directory
    /// (or uses the directory provided at server startup).
    path: Option<String>,
}

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

    /// Enable all selectors at once: task_overdue, task_in_progress, event_span,
    /// created_at, updated_at. Produces a Bullet Journal-style log view when combined
    /// with a period. Individual selectors can still be set on top.
    active: Option<bool>,

    /// Include incomplete tasks whose due date falls within (or before) the period.
    /// Without period: include tasks whose due date is in the past and closed_at is absent.
    task_overdue: Option<bool>,

    /// Include incomplete tasks that were started within (or before) the period.
    /// Without period: include all tasks that have started_at set and closed_at absent.
    task_in_progress: Option<bool>,

    /// Include tasks that have not been started yet (started_at and closed_at both absent).
    /// Period is not applied to this filter.
    task_unstarted: Option<bool>,

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
    entry: EntryRef,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct EntryNewParams {
    /// Title of the entry — written into the frontmatter and used to generate the filename slug
    title: Option<String>,
    /// Body content (Markdown)
    body: Option<String>,
    /// Parent entry
    parent: Option<EntryRef>,
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
struct EntryModifyParams {
    entry: EntryRef,
    /// New title
    title: Option<String>,
    /// New body content (Markdown). Replaces the existing body.
    body: Option<String>,
    /// New parent entry. Omit to leave unchanged; pass null to remove the parent.
    #[serde(default)]
    parent: UpdateOption<EntryRef>,
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
    entry: EntryRef,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct EntryFixParams {
    entry: EntryRef,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct EntryRemoveParams {
    entry: EntryRef,
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
        title: None,
        body: None,
        parent: UpdateOption::Unchanged,
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

fn build_filter(p: &EntryListParams, week_start: WeekStart) -> anyhow::Result<EntryFilter> {
    let parse = |s: &str| parse_period(s, week_start).map_err(anyhow::Error::msg);
    let base = if p.active.unwrap_or(false) { FieldSelector::active() } else { FieldSelector::default() };
    Ok(EntryFilter {
        period: p.period.as_deref().map(parse).transpose()?,
        fields: FieldSelector {
            task_overdue:     base.task_overdue     || p.task_overdue.unwrap_or(false),
            task_in_progress: base.task_in_progress || p.task_in_progress.unwrap_or(false),
            task_unstarted:   base.task_unstarted   || p.task_unstarted.unwrap_or(false),
            event_span:       base.event_span       || p.event_span.unwrap_or(false),
            created_at:       base.created_at       || p.created_at.unwrap_or(false),
            updated_at:       base.updated_at       || p.updated_at.unwrap_or(false),
        },
        task_status: p.task_status.clone().unwrap_or_default(),
        tags: p.tags.clone().unwrap_or_default(),
        sort_by: p.sort_by.as_deref()
            .map(|s| s.parse::<SortField>().map_err(anyhow::Error::msg))
            .transpose()?
            .unwrap_or_default(),
        sort_order: p.sort_order.as_deref()
            .map(|s| s.parse::<SortOrder>().map_err(anyhow::Error::msg))
            .transpose()?
            .unwrap_or_default(),
    })
}

// ── tool implementations ──────────────────────────────────────────────────────

#[tool_router]
impl ArchelonServer {
    #[tool(description = "Open (or reopen) a journal workspace. \
        Loads the journal directory and its SQLite cache into memory so subsequent \
        operations reuse the same connection without reopening files on each call. \
        Call this when switching to a different journal or to refresh the in-memory state.")]
    fn journal_open(&self, Parameters(p): Parameters<JournalOpenParams>) -> Result<String, String> {
        (|| -> anyhow::Result<String> {
            let dir = p.path.as_deref().map(Path::new);
            let (root, count) = self.reload_state(dir)?;
            Ok(format!("opened: {root}\n{count} entries indexed"))
        })()
        .map_err(|e| e.to_string())
    }

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

            let config = toml::to_string_pretty(&archelon_core::journal::JournalConfig::default())
                .context("failed to serialize default config")?;
            std::fs::write(archelon_dir.join("config.toml"), config)
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
        Use field selectors (task_overdue, task_in_progress, event_span, created_at, updated_at) \
        to restrict which conditions apply; omitting all selectors applies the period to all \
        timestamp fields (OR). Without a period, field selectors filter entries where that \
        condition is met. event_span uses interval-overlap semantics so in-progress events are \
        included. task_status and tags are independent AND filters.")]
    fn entry_list(&self, Parameters(p): Parameters<EntryListParams>) -> Result<String, String> {
        (|| -> anyhow::Result<String> {
            let filter = build_filter(&p, self.week_start())?;
            let has_filter = filter.has_any_filter();
            let entries = self.with_state(|s| ops::list_entries(s, &filter).map_err(Into::into))?;
            let records: Vec<EntryListItem> = entries
                .iter()
                .map(|(entry, match_flags)| EntryListItem {
                    entry: entry.clone(),
                    match_flags: if has_filter { Some(match_flags.clone()) } else { None },
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
            let filter = build_filter(&p, self.week_start())?;
            let entries = self.with_state(|s| ops::list_entries(s, &filter).map_err(Into::into))?;
            let roots = ops::build_entry_tree(entries);
            Ok(serde_json::to_string_pretty(&roots)?)
        })()
        .map_err(|e| e.to_string())
    }

    #[tool(description = "Show the contents of a journal entry by ID or file path")]
    fn entry_show(&self, Parameters(p): Parameters<EntryShowParams>) -> Result<String, String> {
        (|| -> anyhow::Result<String> {
            let path = self.with_state(|s| {
                ops::resolve_entry(&p.entry, &s.conn).map_err(Into::into)
            })?;
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
            let fields = EntryFields {
                title: p.title,
                body: p.body,
                parent: match p.parent {
                    Some(r) => UpdateOption::Set(r),
                    None => UpdateOption::Unchanged,
                },
                ..fields
            };
            self.with_state(|s| {
                s.sync()?;
                let dest = ops::create_entry(s, fields)?;
                let _ = cache::upsert_entry_from_path(&s.conn, &dest);
                Ok(format!("created: {}", dest.display()))
            })
        })()
        .map_err(|e| e.to_string())
    }

    #[tool(description = "Update frontmatter fields of an existing journal entry")]
    fn entry_modify(&self, Parameters(p): Parameters<EntryModifyParams>) -> Result<String, String> {
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
            let fields = EntryFields {
                title: p.title,
                body: p.body,
                parent: p.parent,
                ..fields
            };
            self.with_state(|s| {
                s.sync()?;
                let path = ops::resolve_entry(&p.entry, &s.conn)?;
                let msg = if let Some(new_path) = ops::update_entry(&path, &s.conn, fields)? {
                    let _ = cache::upsert_entry_from_path(&s.conn, &new_path);
                    format!("updated and renamed: {}", new_path.display())
                } else {
                    let _ = cache::upsert_entry_from_path(&s.conn, &path);
                    format!("updated: {}", path.display())
                };
                Ok(msg)
            })
        })()
        .map_err(|e| e.to_string())
    }

    #[tool(description = "Check whether an entry's frontmatter and filename are valid. \
        Returns 'ok' or a list of issues (e.g. filename mismatch).")]
    fn entry_check(&self, Parameters(p): Parameters<EntryCheckParams>) -> Result<String, String> {
        (|| -> anyhow::Result<String> {
            let path = self.with_state(|s| {
                ops::resolve_entry(&p.entry, &s.conn).map_err(Into::into)
            })?;
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

    #[tool(description = "Normalize an entry: sync closed_at, update updated_at, and rename the file \
        to match its frontmatter ID and title/slug. \
        Reports the rename or confirms the filename is already correct.")]
    fn entry_fix(&self, Parameters(p): Parameters<EntryFixParams>) -> Result<String, String> {
        (|| -> anyhow::Result<String> {
            let path = self.with_state(|s| {
                ops::resolve_entry(&p.entry, &s.conn).map_err(Into::into)
            })?;
            match ops::fix_entry(&path)? {
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
            self.with_state(|s| {
                s.sync()?;
                let path = ops::resolve_entry(&p.entry, &s.conn)?;
                ops::remove_entry(&path)?;
                let _ = cache::remove_from_cache(&s.conn, &path);
                Ok(format!("removed: {}", path.display()))
            })
        })()
        .map_err(|e| e.to_string())
    }

    #[tool(description = "Show cache location, schema version, and entry/tag counts.")]
    fn cache_info(&self, _: Parameters<serde_json::Value>) -> Result<String, String> {
        (|| -> anyhow::Result<String> {
            self.with_state(|s| {
                let info = s.cache_info()?;
                Ok(format!(
                    "path: {}\nschema version: v{} (app: v{})\nfiles tracked: {}\nentries: {}\nunique tags: {}",
                    info.db_path.display(),
                    info.schema_version,
                    cache::SCHEMA_VERSION,
                    info.file_count,
                    info.entry_count,
                    info.unique_tag_count,
                ))
            })
        })()
        .map_err(|e| e.to_string())
    }

    #[tool(description = "Incrementally sync the cache with the current journal state. \
        Re-indexes files whose mtime has changed and removes entries for deleted files. \
        Returns the number of entries in the cache after sync.")]
    fn cache_sync(&self, _: Parameters<serde_json::Value>) -> Result<String, String> {
        (|| -> anyhow::Result<String> {
            self.with_state(|s| {
                s.sync()?;
                let info = s.cache_info()?;
                Ok(format!("synced: {} entries", info.entry_count))
            })
        })()
        .map_err(|e| e.to_string())
    }

    #[tool(description = "Rebuild the local SQLite cache from scratch. \
        Use this after updating archelon when the schema has changed, \
        or when the cache has become inconsistent. \
        Returns the number of entries indexed.")]
    fn cache_rebuild(&self, _: Parameters<serde_json::Value>) -> Result<String, String> {
        (|| -> anyhow::Result<String> {
            let mut guard = self.state.lock().unwrap();
            let journal_root = match guard.as_ref() {
                Some(s) => s.journal.root.clone(),
                None => self.open_default_journal()?.root,
            };
            let journal = Journal::from_root(journal_root)?;
            let state = JournalState::rebuild(journal)?;
            state.sync()?;
            let info = state.cache_info()?;
            *guard = Some(state);
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
                 Use journal_open to load a workspace into memory (auto-opens on first use). \
                 Use entry_list to browse entries, entry_show to read one, \
                 entry_new to create, and entry_modify to update metadata."
                    .to_owned(),
            )
    }
}

// ── main ──────────────────────────────────────────────────────────────────────

#[tokio::main]
pub async fn run(journal_dir: Option<&Path>) -> anyhow::Result<()> {
    // Log to stderr so stdout remains clean for the MCP JSON-RPC protocol
    tracing_subscriber::fmt().with_writer(std::io::stderr).init();

    let server = ArchelonServer::new(journal_dir.map(|x| x.to_path_buf()));
    let service = server.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
