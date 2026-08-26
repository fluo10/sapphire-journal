//! テスト用に journal を 1 つ作り、鍵を 1 本発行してサーバを組み立てる。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode, header};
use http_body_util::BodyExt as _;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt as _;

pub struct Harness {
    _tmp: tempfile::TempDir,
    token: String,
    ws: String,
    router: Router,
    /// Set once in `spawn()` by completing the real MCP handshake against
    /// `/mcp`. Reused for every subsequent `tools/call`.
    mcp_session: String,
    next_mcp_id: AtomicI64,
}

pub async fn spawn() -> Harness {
    let tmp = tempfile::tempdir().unwrap();
    let journal_dir = tmp.path().join("journal");
    sapphire_journal_core::init_app_context();
    init_journal(&journal_dir);

    let keys_path = tmp.path().join("keys.toml");
    let mut store = sapphire_framework::remote_server::KeyStore::load(&keys_path).unwrap();
    let token = store
        .generate(sapphire_journal_server::keys::TOKEN_PREFIX, Some("test".into()), None)
        .unwrap()
        .token;

    let journal_state = sapphire_journal_server::serve::open_journal_state(&journal_dir).unwrap();
    let state = sapphire_journal_server::serve::build_state(
        &journal_dir,
        &keys_path,
        Arc::clone(&journal_state),
    )
    .unwrap();
    let ws = sapphire_journal_server::serve::workspace_id(&journal_dir).unwrap();
    let router = sapphire_journal_server::serve::build_router(
        Arc::clone(&state),
        journal_state,
        &journal_dir,
        CancellationToken::new(),
    )
    .unwrap();

    let mut h = Harness {
        _tmp: tmp,
        token,
        ws,
        router,
        mcp_session: String::new(),
        next_mcp_id: AtomicI64::new(1),
    };
    h.mcp_session = h.mcp_initialize().await;
    h
}

/// `.sapphire-journal/` を掘って journal にする。`ensure_journal` の init 分岐と同じ。
fn init_journal(root: &std::path::Path) {
    use sapphire_journal_core::journal::JournalConfig;
    let journal_dir = root.join(".sapphire-journal");
    std::fs::create_dir_all(&journal_dir).unwrap();
    std::fs::write(
        journal_dir.join("config.toml"),
        toml::to_string_pretty(&JournalConfig::default()).unwrap(),
    )
    .unwrap();
    std::fs::write(journal_dir.join(".gitignore"), "cache/\n").unwrap();
}

/// Streamable HTTP wraps every request-scoped response in SSE, and the
/// default config sends an empty "priming" event (SEP-1699) before the real
/// one. Skip blank `data:` frames and return the first one that parses.
fn first_sse_json_message(body: &str) -> serde_json::Value {
    for event in body.split("\n\n") {
        let data: String = event
            .lines()
            .filter_map(|l| l.strip_prefix("data:"))
            .map(str::trim_start)
            .collect::<Vec<_>>()
            .join("\n");
        if data.is_empty() {
            continue;
        }
        if let Ok(v) = serde_json::from_str(&data) {
            return v;
        }
    }
    panic!("no JSON-RPC message found in SSE body: {body:?}");
}

impl Harness {
    pub fn router(&self) -> Router {
        self.router.clone()
    }
    pub fn ws(&self) -> &str {
        &self.ws
    }

    /// 鍵つきで JSON-RPC を 1 回叩く。
    pub async fn rpc(&self, method: &str, params: serde_json::Value) -> serde_json::Value {
        let body = serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": method, "params": params
        });
        let response = self
            .router()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/rpc")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, format!("Bearer {}", self.token))
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    fn next_mcp_id(&self) -> i64 {
        self.next_mcp_id.fetch_add(1, Ordering::Relaxed)
    }

    /// One authenticated POST to `/mcp`, through the exact `Router`
    /// `build_router` assembles — the same one a real MCP client (or `/rpc`,
    /// for the auth layer) talks to. This is deliberately the *only* way
    /// this harness reaches `SapphireJournalServer`: nothing here builds one
    /// itself, so a write that reaches the change log can only have got
    /// there through `build_router`'s own wiring.
    async fn mcp_post(&self, session: Option<&str>, body: serde_json::Value) -> (StatusCode, HeaderMap, String) {
        let mut builder = Request::builder()
            .method("POST")
            .uri("/mcp")
            .header(header::CONTENT_TYPE, "application/json")
            // Streamable HTTP requires accepting both; see rmcp's handle_post.
            .header(header::ACCEPT, "application/json, text/event-stream")
            .header(header::AUTHORIZATION, format!("Bearer {}", self.token))
            // rmcp's DNS-rebinding guard rejects requests with no Host (or an
            // unlisted one); `oneshot()` doesn't synthesize one.
            .header(header::HOST, "localhost");
        if let Some(session) = session {
            builder = builder.header("Mcp-Session-Id", session);
        }
        let response = self
            .router()
            .oneshot(builder.body(Body::from(serde_json::to_vec(&body).unwrap())).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let text = String::from_utf8_lossy(&bytes).into_owned();
        (status, headers, text)
    }

    /// Complete the MCP handshake (`initialize` then `notifications/initialized`)
    /// and return the session id every later `tools/call` must carry.
    async fn mcp_initialize(&self) -> String {
        let (status, headers, body) = self
            .mcp_post(
                None,
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 0,
                    "method": "initialize",
                    "params": {
                        "protocolVersion": "2025-06-18",
                        "capabilities": {},
                        "clientInfo": {"name": "sapphire-journal-server-tests", "version": "0.0.0"}
                    }
                }),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "initialize failed: {body}");
        let session = headers
            .get("mcp-session-id")
            .unwrap_or_else(|| panic!("no Mcp-Session-Id header on initialize response: {body}"))
            .to_str()
            .unwrap()
            .to_owned();
        let msg = first_sse_json_message(&body);
        assert!(msg.get("error").is_none(), "initialize returned an error: {msg:?}");

        let (status, _, body) = self
            .mcp_post(
                Some(&session),
                serde_json::json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            )
            .await;
        assert_eq!(status, StatusCode::ACCEPTED, "notifications/initialized rejected: {body}");

        session
    }

    /// Call an MCP tool over the real `/mcp` endpoint. Returns the tool's
    /// text result — the same string any MCP client would see.
    async fn mcp_call_tool(&self, name: &str, arguments: serde_json::Value) -> String {
        let id = self.next_mcp_id();
        let (status, _, body) = self
            .mcp_post(
                Some(&self.mcp_session),
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "method": "tools/call",
                    "params": {"name": name, "arguments": arguments}
                }),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "tools/call {name} transport failed: {body}");
        let msg = first_sse_json_message(&body);
        if let Some(err) = msg.get("error") {
            panic!("tools/call {name} returned a JSON-RPC error: {err:?}");
        }
        let result = &msg["result"];
        assert_ne!(
            result.get("isError").and_then(|v| v.as_bool()).unwrap_or(false),
            true,
            "tool {name} reported an error: {result:?}"
        );
        result["content"][0]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("tool {name} result has no text content: {result:?}"))
            .to_owned()
    }

    /// `entry_new` over `/mcp`. Returns the created entry's absolute path.
    pub async fn write_entry_through_mcp(&self, title: &str) -> PathBuf {
        let msg = self.mcp_call_tool("entry_new", serde_json::json!({"title": title})).await;
        PathBuf::from(
            msg.strip_prefix("created: ")
                .unwrap_or_else(|| panic!("unexpected entry_new response: {msg}")),
        )
    }

    /// `entry_modify` over `/mcp`, changing only the title. Asserts the
    /// change actually renamed the file (`update_entry` renames on a
    /// slug-changing title) — a vacuous "updated: ..." with no rename would
    /// silently defeat the point of the caller's test.
    pub async fn retitle_through_mcp(&self, path: &Path, new_title: &str) {
        let msg = self
            .mcp_call_tool(
                "entry_modify",
                serde_json::json!({
                    "entry": {"path": path.to_string_lossy()},
                    "title": new_title,
                }),
            )
            .await;
        assert!(
            msg.starts_with("updated and renamed:"),
            "expected a rename, got: {msg}"
        );
    }
}
