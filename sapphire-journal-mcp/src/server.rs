use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::Context as _;
use sapphire_journal_core::{
    cache,
    entry_ref::EntryRef,
    journal::{Journal, JournalConfig},
    ops::{self, EntryFields, EntryListItem, UpdateOption},
    parser::read_entry,
    state as core_state,
    text_input::{fields as core_fields, filter as core_filter},
    user_config::UserConfig,
    JournalState,
};
use tokio::time;
use rmcp::{
    ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::*,
    schemars,
    tool, tool_router,
    transport::stdio,
};
use sapphire_journal_core::{FtsQuery, VectorQuery};
use serde::Deserialize;

// ── server struct ─────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct SapphireJournalServer {
    /// Cached journal + SQLite connection, shared across tool calls.
    state: Arc<Mutex<JournalState>>,
    tool_router: ToolRouter<Self>,
}

impl std::fmt::Debug for SapphireJournalServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SapphireJournalServer").finish_non_exhaustive()
    }
}

impl SapphireJournalServer {
    pub fn new(state: JournalState) -> Self {
        Self::from_shared(Arc::new(Mutex::new(state)))
    }

    /// Build a server that shares an existing `Arc<Mutex<JournalState>>`.
    /// Used by the HTTP transport, where each session spawns a fresh server
    /// instance via a factory but all instances must operate on the same state.
    pub fn from_shared(state: Arc<Mutex<JournalState>>) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
        }
    }

    /// Shared state — exposed so callers (HTTP transport setup) can spawn
    /// background tasks like periodic git sync against the same journal.
    pub fn shared_state(&self) -> Arc<Mutex<JournalState>> {
        Arc::clone(&self.state)
    }

    fn with_state<F, T>(&self, f: F) -> anyhow::Result<T>
    where
        F: FnOnce(&JournalState) -> anyhow::Result<T>,
    {
        let guard = self.state.lock().unwrap();
        f(&guard)
    }
}

// ── parameter structs ─────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct EntryListParams {
    /// Period to match against timestamp fields.
    /// When no field selectors are set, the period applies to all fields (OR).
    ///
    /// Accepted formats: today | yesterday | tomorrow |
    /// this_week | last_week | next_week |
    /// this_month | last_month | next_month | none |
    /// YYYY | YYYY-MM | YYYY-Www |
    /// YYYY-MM-DD | YYYY-MM-DD/YYYY-MM-DD | YYYY-MM-DDTHH:MM/YYYY-MM-DDTHH:MM
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

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct EntrySearchParams {
    /// Search query text. Supports substring and CJK queries (FTS5 trigram index).
    query: String,
    /// Maximum number of results to return (default: 10).
    limit: Option<usize>,
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
    use core_fields::{parse_optional_datetime, parse_optional_datetime_end, parse_tags_csv};
    Ok(EntryFields {
        title: None,
        body: None,
        parent: UpdateOption::Unchanged,
        slug,
        tags: parse_tags_csv(tags.as_deref()),
        task_due: parse_optional_datetime_end(task_due)?,
        task_status,
        task_started_at: parse_optional_datetime(task_started_at)?,
        task_closed_at: parse_optional_datetime(task_closed_at)?,
        event_start: parse_optional_datetime(event_start)?,
        event_end: parse_optional_datetime_end(event_end)?,
    })
}

impl<'a> From<&'a EntryListParams> for core_filter::FilterInputs<'a> {
    fn from(p: &'a EntryListParams) -> Self {
        Self {
            period: p.period.as_deref(),
            active: p.active.unwrap_or(false),
            task_overdue: p.task_overdue.unwrap_or(false),
            task_in_progress: p.task_in_progress.unwrap_or(false),
            task_unstarted: p.task_unstarted.unwrap_or(false),
            event_span: p.event_span.unwrap_or(false),
            created_at: p.created_at.unwrap_or(false),
            updated_at: p.updated_at.unwrap_or(false),
            task_status: p.task_status.as_deref().unwrap_or(&[]),
            tags: p.tags.as_deref().unwrap_or(&[]),
            sort_by: p.sort_by.as_deref(),
            sort_order: p.sort_order.as_deref(),
        }
    }
}

// ── tool implementations ──────────────────────────────────────────────────────

#[tool_router]
impl SapphireJournalServer {
    #[tool(description = "List journal entries as JSON. \
        Use `period` to specify a time range. \
        Use field selectors (task_overdue, task_in_progress, event_span, created_at, updated_at) \
        to restrict which conditions apply; omitting all selectors applies the period to all \
        timestamp fields (OR). Without a period, field selectors filter entries where that \
        condition is met. event_span uses interval-overlap semantics so in-progress events are \
        included. task_status and tags are independent AND filters.")]
    fn entry_list(&self, Parameters(p): Parameters<EntryListParams>) -> Result<String, String> {
        (|| -> anyhow::Result<String> {
            let filter = core_filter::build_filter(core_filter::FilterInputs::from(&p))?;
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
            let filter = core_filter::build_filter(core_filter::FilterInputs::from(&p))?;
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
                let conn = s.open_conn()?;
                ops::resolve_entry(&p.entry, &conn).map_err(Into::into)
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
                if let Ok(conn) = s.open_conn() {
                    let _ = cache::upsert_entry_from_path(&conn, &dest, s.retrieve_db());
                }
                let _ = s.on_file_updated(&dest);
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
                && matches!(p.parent, UpdateOption::Unchanged)
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
                let conn = s.open_conn()?;
                let path = ops::resolve_entry(&p.entry, &conn)?;
                let msg = if let Some(new_path) = ops::update_entry(&path, &conn, fields)? {
                    let _ = cache::upsert_entry_from_path(&conn, &new_path, s.retrieve_db());
                    // File was renamed: remove old path and stage new path
                    let _ = s.on_file_deleted(&path);
                    let _ = s.on_file_updated(&new_path);
                    format!("updated and renamed: {}", new_path.display())
                } else {
                    let _ = cache::upsert_entry_from_path(&conn, &path, s.retrieve_db());
                    let _ = s.on_file_updated(&path);
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
                let conn = s.open_conn()?;
                ops::resolve_entry(&p.entry, &conn).map_err(Into::into)
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
                let conn = s.open_conn()?;
                ops::resolve_entry(&p.entry, &conn).map_err(Into::into)
            })?;
            match ops::fix_entry(&path)? {
                Some(new_path) => {
                    self.with_state(|s| {
                        let _ = s.on_file_deleted(&path);
                        let _ = s.on_file_updated(&new_path);
                        Ok(())
                    })?;
                    Ok(format!(
                        "renamed: {} → {}",
                        path.file_name().unwrap_or_default().to_string_lossy(),
                        new_path.file_name().unwrap_or_default().to_string_lossy(),
                    ))
                }
                None => {
                    self.with_state(|s| {
                        let _ = s.on_file_updated(&path);
                        Ok(())
                    })?;
                    Ok(format!("ok: {} (already correct)", path.display()))
                }
            }
        })()
        .map_err(|e| e.to_string())
    }

    #[tool(description = "Delete an entry file from the journal")]
    fn entry_remove(&self, Parameters(p): Parameters<EntryRemoveParams>) -> Result<String, String> {
        (|| -> anyhow::Result<String> {
            self.with_state(|s| {
                s.sync()?;
                let conn = s.open_conn()?;
                let path = ops::resolve_entry(&p.entry, &conn)?;
                ops::remove_entry(&path)?;
                let _ = cache::remove_from_cache(&conn, &path, s.retrieve_db());
                let _ = s.on_file_deleted(&path);
                Ok(format!("removed: {}", path.display()))
            })
        })()
        .map_err(|e| e.to_string())
    }

    #[tool(description = "Run a full git sync cycle: commit staged changes, fetch and merge from \
        remote, then push. No-op when no git repository is found or sync is disabled.")]
    fn git_sync(&self, _: Parameters<serde_json::Value>) -> Result<String, String> {
        (|| -> anyhow::Result<String> {
            self.with_state(|s| {
                if !s.has_sync_backend() {
                    return Ok("skipped: no sync backend configured".to_owned());
                }
                s.git_sync()?;
                Ok("sync complete".to_owned())
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
        Use this after updating sapphire-journal when the schema has changed, \
        or when the cache has become inconsistent. \
        Returns the number of entries indexed.")]
    fn cache_rebuild(&self, _: Parameters<serde_json::Value>) -> Result<String, String> {
        (|| -> anyhow::Result<String> {
            let mut guard = self.state.lock().unwrap();
            let journal_root = guard.journal.root.clone();
            let journal = Journal::from_root(journal_root)?;
            let state = JournalState::rebuild(journal)?;
            state.sync()?;
            let info = state.cache_info()?;
            *guard = state;
            Ok(format!("rebuilt: {} entries indexed", info.entry_count))
        })()
        .map_err(|e| e.to_string())
    }

    #[tool(description = "Search journal entries. \
        When `cache.embedding.enabled = true` in the user config, uses approximate \
        (vector/semantic) search for more relevant results. \
        Otherwise falls back to full-text search (FTS5 trigram index, supports \
        substring and CJK queries). \
        Returns a JSON array of results ordered by relevance, each with \
        `title`, `path`, and `score`.")]
    fn entry_search(&self, Parameters(p): Parameters<EntrySearchParams>) -> Result<String, String> {
        (|| -> anyhow::Result<String> {
            self.with_state(|s| {
                s.sync()?;

                let limit = p.limit.unwrap_or(10);

                if let Some(embedder) = s.embedder() {
                    // Auto-embed a small number of pending chunks so freshly-synced
                    // entries are included in vector search results.
                    let pending_count = s.retrieve_db().vec_info()
                        .map(|vi| vi.pending_count)
                        .unwrap_or(0);
                    if pending_count > 0 && pending_count <= 50 {
                        let _ = s.retrieve_db().embed_pending(embedder, |_, _| {});
                    }

                    // Vector search.
                    let q = VectorQuery::new(p.query.as_str(), embedder).limit(limit);
                    let results = s.retrieve_db().search_similar(&q)
                        .map_err(anyhow::Error::msg)?;
                    return Ok(serde_json::to_string_pretty(&results)?);
                }

                // Fallback: full-text search.
                let q = FtsQuery::new(&p.query).limit(limit);
                let results = s.retrieve_db().search_fts(&q)
                    .map_err(anyhow::Error::msg)?;
                Ok(serde_json::to_string_pretty(&results)?)
            })
        })()
        .map_err(|e| e.to_string())
    }
}

#[rmcp::tool_handler(router = self.tool_router)]
impl ServerHandler for SapphireJournalServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder().enable_tools().build(),
        )
        .with_instructions(
            "Sapphire Journal is a Markdown-based journal/task manager. \
             Use entry_list to browse entries, entry_show to read one, \
             entry_new to create, and entry_modify to update metadata."
                .to_owned(),
        )
    }
}

// ── startup helpers ───────────────────────────────────────────────────────────

/// Resolve the journal at `dir` (or via upward search from CWD when `dir` is None).
///
/// When `init` is true, the target path (literal `dir` or CWD — no upward search)
/// is created as needed and turned into a sapphire-journal if it isn't one already.
/// Existing journals are reused as-is.
pub(crate) fn ensure_journal(dir: Option<&Path>, init: bool) -> anyhow::Result<Journal> {
    if init {
        let target: PathBuf = match dir {
            Some(d) => d.to_path_buf(),
            None => std::env::current_dir().context("failed to read current directory")?,
        };

        if !target.exists() {
            std::fs::create_dir_all(&target)
                .with_context(|| format!("failed to create directory {}", target.display()))?;
        }

        let journal_dir = target.join(".sapphire-journal");
        if !journal_dir.exists() {
            std::fs::create_dir(&journal_dir)
                .context("failed to create .sapphire-journal directory")?;
            let config = toml::to_string_pretty(&JournalConfig::default())
                .context("failed to serialize default config")?;
            std::fs::write(journal_dir.join("config.toml"), config)
                .context("failed to write .sapphire-journal/config.toml")?;
            std::fs::write(journal_dir.join(".gitignore"), "cache/\n")
                .context("failed to write .sapphire-journal/.gitignore")?;
            tracing::info!("initialized sapphire-journal in {}", target.display());
        }

        Journal::from_root(target).context("failed to open journal after init")
    } else {
        match dir {
            Some(d) => Journal::from_root(d.to_path_buf()).with_context(|| {
                format!(
                    "not a sapphire-journal: {} — pass --init to create one",
                    d.display()
                )
            }),
            None => Journal::find().context(
                "no sapphire-journal found in the current directory or any parent \
                 — pass --init to create one",
            ),
        }
    }
}

/// Open a [`JournalState`] for the given directory and run any embedder
/// bootstrap configured in the user config.
///
/// Used by both transports (stdio, HTTP). Caller is responsible for setting
/// up tracing — stdio writes logs to stderr (so stdout stays clean for the
/// JSON-RPC protocol), HTTP can use whatever subscriber the host process
/// configured.
pub(crate) fn prepare_state(
    journal_dir: Option<&Path>,
    init: bool,
) -> anyhow::Result<JournalState> {
    let journal = ensure_journal(journal_dir, init)?;
    let state = JournalState::open(journal)?;
    state.sync()?;

    let config = UserConfig::load()?;
    if config
        .cache
        .retrieve
        .embedding
        .as_ref()
        .map(|e| e.enabled)
        .unwrap_or(false)
    {
        tokio::task::block_in_place(|| core_state::bootstrap_embedder(&state, &config))?;
    }
    Ok(state)
}

/// Spawn the periodic git sync task on the current tokio runtime when the
/// user config sets a non-zero `sync_interval_minutes`. Returns `None` when
/// disabled. The returned [`JoinHandle`] can be `.abort()`ed by the caller
/// (e.g. when the HTTP transport shuts down) to stop the task.
pub(crate) fn spawn_periodic_git_sync(
    state: Arc<Mutex<JournalState>>,
) -> Option<tokio::task::JoinHandle<()>> {
    let interval = UserConfig::load().ok().and_then(|c| c.sync_interval())?;
    Some(tokio::spawn(async move {
        let mut ticker = time::interval(interval);
        ticker.tick().await; // skip the first immediate tick
        loop {
            ticker.tick().await;
            let guard = state.lock().unwrap();
            if let Err(e) = guard.git_sync() {
                eprintln!("[sapphire-journal] periodic git sync failed: {e}");
            }
            drop(guard);
        }
    }))
}

// ── stdio entry point ─────────────────────────────────────────────────────────

#[tokio::main]
pub async fn run(journal_dir: Option<&Path>, init: bool) -> anyhow::Result<()> {
    // Log to stderr so stdout remains clean for the MCP JSON-RPC protocol
    tracing_subscriber::fmt().with_writer(std::io::stderr).init();

    let state = prepare_state(journal_dir, init)?;
    let server = SapphireJournalServer::new(state);
    let _sync_handle = spawn_periodic_git_sync(server.shared_state());

    let service = server.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
