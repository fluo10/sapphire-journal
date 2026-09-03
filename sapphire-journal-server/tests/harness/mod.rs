//! テスト用に journal を 1 つ作り、鍵を 1 本発行してサーバを組み立てる。
//!
//! このファイルは `tests/*.rs` の**すべて**にコンパイルされるので、どのヘルパ
//! も「自分を使っていないテストバイナリ」から見れば未使用になる。`dead_code`
//! を許可しておかないと、1 つのテストでしか使わないヘルパを足すたびに他の
//! バイナリで警告が出る。
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode, header};
use http_body_util::BodyExt as _;
use sapphire_framework::remote_server::{ReconcileReport, WsStore};
use sapphire_journal_core::journal_state::JournalState;
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
    /// journal のルート。Task 8 のテストは、通知を経由しない書き込みをここに
    /// 直接植える。
    pub journal_dir: PathBuf,
    /// `build_router` が開いたのと同じ `WsStore`(`ServerState::workspace` は
    /// ws ごとにメモ化する)。`tick_once` はここではなく change log を更新する
    /// ので、テストの検証も change log(`workspace.snapshot`)側で行うこと。
    store: Arc<WsStore>,
    journal_state: Arc<Mutex<JournalState>>,
    /// Every `/mcp` request this harness sends carries this as its `Host`.
    /// rmcp rejects a `Host` that is not on the router's allowlist, so this
    /// is what makes the allowlist observable from a test.
    mcp_host: String,
}

pub async fn spawn() -> Harness {
    spawn_with_allowed_hosts(&[], "localhost").await
}

/// Build the server with `allowed_hosts` widening the MCP `Host` allowlist,
/// and send every `/mcp` request with `Host: {mcp_host}`.
///
/// `spawn()` is this with an empty list and a loopback `Host` — i.e. rmcp's
/// own default, which is what a server bound to `127.0.0.1` wants.
pub async fn spawn_with_allowed_hosts(allowed_hosts: &[String], mcp_host: &str) -> Harness {
    let tmp = tempfile::tempdir().unwrap();
    let journal_dir = tmp.path().join("journal");
    sapphire_journal_core::init_app_context();
    init_journal(&journal_dir);

    let keys_path = tmp.path().join("keys.toml");
    let token = mint_device_key(&journal_dir, &keys_path, "test");

    let journal_state = sapphire_journal_server::serve::open_journal_state(&journal_dir).unwrap();
    let state = sapphire_journal_server::serve::build_state(
        &journal_dir,
        &keys_path,
        Arc::clone(&journal_state),
    )
    .unwrap();
    let ws = sapphire_journal_server::serve::workspace_id(&journal_dir).unwrap();
    let device_auth = std::sync::Arc::new(
        sapphire_journal_server::device_auth::DeviceAuth::load(
            &keys_path,
            &sapphire_journal_server::serve::default_devices_path(&journal_dir).unwrap(),
        )
        .unwrap(),
    );
    let router = sapphire_journal_server::serve::build_router(
        Arc::clone(&state),
        Arc::clone(&journal_state),
        CancellationToken::new(),
        allowed_hosts,
        device_auth,
    )
    .unwrap();
    // `build_router` が `state.workspace(&ws)` で既に開いている。`workspace`
    // はメモ化されるので、ここで取り直しても同じインスタンス — change log が
    // 2 本になる心配はない。
    let store = state.workspace(&ws).unwrap();

    let mut h = Harness {
        _tmp: tmp,
        token,
        ws,
        router,
        mcp_session: String::new(),
        next_mcp_id: AtomicI64::new(1),
        journal_dir,
        store,
        journal_state,
        mcp_host: mcp_host.to_owned(),
    };
    h.mcp_session = h.mcp_initialize().await;
    h
}

/// `.sapphire-journal/` を掘って journal にする。`ensure_journal` の init 分岐と同じ。
///
/// `pub` なのは、鍵を 1 本も発行しない状態から始めたいテスト（`run` が起動を
/// 拒否することを確かめるテストなど）が `spawn()` を経由せずに journal だけ
/// 欲しいことがあるため。
pub fn init_journal(root: &std::path::Path) {
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

/// `device add` を通してデバイス行と鍵を作り、トークンを返す。
///
/// 本物のサブコマンドを通す。トークンは stdout に出て掴まえられないので、
/// 鍵ファイルから読み直す —— 運用者が「なくしたら鍵ファイルを見ればいい」と
/// 案内されているのと同じ経路。
pub fn mint_device_key(
    journal_dir: &std::path::Path,
    keys_path: &std::path::Path,
    name: &str,
) -> String {
    let devices_path = sapphire_journal_server::serve::default_devices_path(journal_dir).unwrap();
    let users_path = sapphire_journal_server::serve::default_users_path(journal_dir).unwrap();
    sapphire_journal_server::cli_device::run_device(
        sapphire_journal_server::cli_device::DeviceCommand::Add {
            name: name.to_owned(),
            description: None,
            user: None,
            expires_in: None,
        },
        &devices_path,
        &users_path,
        keys_path,
    )
    .unwrap();
    let store = sapphire_framework::remote_server::KeyStore::load(keys_path).unwrap();
    store
        .entries()
        .iter()
        .find(|k| k.label.as_deref() == Some(name))
        .expect("いま発行した鍵が見つからない")
        .token
        .clone()
}

/// エントリのフィクスチャを、アプリ本体と同じ `parse_entry` → `render_entry`
/// を通して組み立てる。
///
/// **手書きの frontmatter を使わないこと。** `tick.rs` と `crlf.rs` はどちらも
/// 手書きで、どちらも id を間違えていた（`01J9` は 4 文字、
/// `01J000000000000000000000` は 24 文字。`GrainId` は 7 文字ちょうどしか
/// 受け付けない）。id が無効だと `read_entry` が落ち、そのファイルは journal の
/// キャッシュに一切載らない —— 「キャッシュ越しに読めること」を確かめたつもり
/// のテストが、パーサが完全に壊れていても通る状態になっていた。
///
/// ここを通せば、無効な id や壊れた frontmatter はフィクスチャを作る時点で
/// panic するので、同じ間違いが静かに通ることはない。
pub fn render_entry_fixture(id: &str, title: &str, body: &str) -> String {
    let src = format!("---\nid: '{id}'\ntitle: {title}\n---\n\n{body}\n");
    let path = PathBuf::from(format!("{id}.md"));
    let entry = sapphire_journal_core::parser::parse_entry(&path, &src)
        .unwrap_or_else(|e| panic!("フィクスチャが entry として読めない (id {id:?}): {e}"));
    sapphire_journal_core::parser::render_entry(&entry)
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

    /// `path`（journal 内の絶対パス）を、change log / RPC が使うワークスペース
    /// 相対の POSIX パスに直す。
    pub fn relative(&self, path: &Path) -> String {
        path.strip_prefix(&self.journal_dir)
            .unwrap_or_else(|_| panic!("{path:?} is not under the journal dir {:?}", self.journal_dir))
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/")
    }

    /// [`render_entry_fixture`] を journal 内の `rel` に書く。返すのは絶対パス。
    pub fn write_entry_fixture(&self, rel: &str, id: &str, title: &str, body: &str) -> PathBuf {
        self.write_raw(rel, &render_entry_fixture(id, title, body))
    }

    /// 同じものを CRLF で書く。`render_entry` は LF しか出さないので、
    /// チェックアウト時に `core.autocrlf=true` が行うのと同じ変換をここで行う。
    pub fn write_entry_fixture_crlf(
        &self,
        rel: &str,
        id: &str,
        title: &str,
        body: &str,
    ) -> PathBuf {
        let lf = render_entry_fixture(id, title, body);
        self.write_raw(rel, &lf.replace('\n', "\r\n"))
    }

    fn write_raw(&self, rel: &str, contents: &str) -> PathBuf {
        let path = self
            .journal_dir
            .join(rel.replace('/', std::path::MAIN_SEPARATOR_STR));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, contents).unwrap();
        path
    }

    /// journal のキャッシュがこの id に対して持っているタイトル。
    ///
    /// change log は本文を不透明なテキストとして持つだけなので、そちらを見ても
    /// frontmatter が解釈できたかは分からない。**parse を経た値**を確かめたい
    /// テストはここを使うこと。
    pub fn cached_title(&self, id: &str) -> Option<String> {
        let guard = self.journal_state.lock().unwrap();
        let conn = guard.open_conn().ok()?;
        let id: grain_id::GrainId = id.parse().ok()?;
        sapphire_journal_core::cache::find_entry_by_id(&conn, id)
            .ok()
            .map(|e| e.frontmatter.title)
    }

    /// Task 8 の `tick_once`(journal の sync + `WsStore::reconcile`)を 1 回分、
    /// 同期的に走らせる。
    pub async fn tick_once(&self) -> ReconcileReport {
        sapphire_journal_server::serve::tick_once(&self.store, &self.journal_state).unwrap()
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
            // unlisted one); `oneshot()` doesn't synthesize one. Which Host
            // this is comes from `spawn_with_allowed_hosts` — the whole point
            // of `allowed_hosts.rs` is that a non-loopback one gets through.
            .header(header::HOST, &self.mcp_host);
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

    /// An authenticated `initialize` to `/mcp` carrying `host` as its `Host`
    /// header, returning only the transport status.
    ///
    /// The token is valid, so a non-`OK` status here is rmcp's `Host`
    /// allowlist talking and not the auth layer.
    pub async fn mcp_initialize_status_with_host(&self, host: &str) -> (StatusCode, String) {
        let response = self
            .router()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mcp")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::ACCEPT, "application/json, text/event-stream")
                    .header(header::AUTHORIZATION, format!("Bearer {}", self.token))
                    .header(header::HOST, host)
                    .body(Body::from(
                        serde_json::to_vec(&serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": 0,
                            "method": "initialize",
                            "params": {
                                "protocolVersion": "2025-06-18",
                                "capabilities": {},
                                "clientInfo": {"name": "host-allowlist-probe", "version": "0.0.0"}
                            }
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8_lossy(&bytes).into_owned())
    }

    /// The same probe against `/rpc`, so a test can show that the two routes
    /// disagree about the very same request.
    pub async fn rpc_status_with_host(&self, host: &str) -> StatusCode {
        let body = serde_json::json!({
            "jsonrpc": "2.0", "id": 1,
            "method": "workspace.snapshot", "params": {"ws": self.ws()}
        });
        self.router()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/rpc")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, format!("Bearer {}", self.token))
                    .header(header::HOST, host)
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap()
            .status()
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

    /// `entry_modify` over `/mcp`, changing only the title. Returns the new
    /// path. Asserts the change actually renamed the file (`update_entry`
    /// renames on a slug-changing title) — a vacuous "updated: ..." with no
    /// rename would silently defeat the point of the caller's test.
    pub async fn retitle_through_mcp(&self, path: &Path, new_title: &str) -> PathBuf {
        let msg = self
            .mcp_call_tool(
                "entry_modify",
                serde_json::json!({
                    "entry": {"path": path.to_string_lossy()},
                    "title": new_title,
                }),
            )
            .await;
        let new_path = msg
            .strip_prefix("updated and renamed: ")
            .unwrap_or_else(|| panic!("expected a rename, got: {msg}"));
        PathBuf::from(new_path)
    }

    /// `resolve_duplicates` の退避先。journal ルートの**外**（cache dir 配下）
    /// にあるので、ここに入ったファイルは同期に出ない。
    pub fn quarantine_dir(&self) -> PathBuf {
        self.journal_state
            .lock()
            .unwrap()
            .journal
            .cache_dir()
            .unwrap()
            .join("quarantine")
    }

    /// 退避されたファイルの一覧（ファイル名, 中身）。
    pub fn quarantined(&self) -> Vec<(String, String)> {
        let dir = self.quarantine_dir();
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return Vec::new();
        };
        entries
            .filter_map(|e| e.ok())
            .map(|e| {
                (
                    e.file_name().to_string_lossy().into_owned(),
                    std::fs::read_to_string(e.path()).unwrap_or_default(),
                )
            })
            .collect()
    }
}
