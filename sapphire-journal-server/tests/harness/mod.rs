//! テスト用に journal を 1 つ作り、鍵を 1 本発行してサーバを組み立てる。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Body;
use axum::http::{Request, header};
use http_body_util::BodyExt as _;
use sapphire_framework::remote_server::ServerState;
use sapphire_journal_core::JournalState;
use sapphire_journal_core::entry_ref::EntryRef;
use sapphire_journal_core::ops::UpdateOption;
use sapphire_journal_mcp::Parameters;
use sapphire_journal_mcp::server::{EntryModifyParams, EntryNewParams};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt as _;

pub struct Harness {
    _tmp: tempfile::TempDir,
    pub journal_dir: PathBuf,
    pub token: String,
    pub ws: String,
    router: Router,
    state: Arc<ServerState>,
    journal_state: Arc<Mutex<JournalState>>,
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
        Arc::clone(&journal_state),
        &journal_dir,
        CancellationToken::new(),
    )
    .unwrap();

    Harness { _tmp: tmp, journal_dir, token, ws, router, state, journal_state }
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

    /// `build_router` の中身と同じ配線 — `state.workspace(&ws)` から得た
    /// `Arc<WsStore>` を `write_observer` に渡し、この harness の journal
    /// state を共有する `SapphireJournalServer` に取り付ける。MCP のツールを
    /// トランスポート越しに叩くのは重すぎるので、この「オブザーバが繋がって
    /// いる入口」を直接呼ぶのがテストの目的に合う — 確かめたいのは MCP の
    /// 書き込みが change log に届くことであって、HTTP を経由することではない。
    fn mcp_server(&self) -> sapphire_journal_mcp::SapphireJournalServer {
        let store = self.state.workspace(&self.ws).unwrap();
        let observer = sapphire_journal_server::serve::write_observer(store, self.journal_dir.clone());
        sapphire_journal_mcp::SapphireJournalServer::from_shared(Arc::clone(&self.journal_state))
            .with_write_observer(observer)
    }

    /// `entry_new` を直接呼んでエントリを 1 件作る。オブザーバ経由で change
    /// log に載っているはずのパスを、呼び出し元が検証できるよう返す。
    pub async fn write_entry_through_mcp(&self, title: &str) -> PathBuf {
        let server = self.mcp_server();
        let msg = server
            .entry_new(Parameters(EntryNewParams {
                title: Some(title.to_owned()),
                body: None,
                parent: None,
                slug: None,
                tags: None,
                task_due: None,
                task_status: None,
                task_started_at: None,
                task_closed_at: None,
                event_start: None,
                event_end: None,
            }))
            .expect("entry_new failed");
        PathBuf::from(
            msg.strip_prefix("created: ")
                .unwrap_or_else(|| panic!("unexpected entry_new response: {msg}")),
        )
    }

    /// `entry_modify` でタイトルを変え、`fix_entry` と同じ経路でファイル名も
    /// 追随させる（`update_entry` はタイトル変更時にリネームする）。
    pub async fn retitle_through_mcp(&self, path: &Path, new_title: &str) {
        let server = self.mcp_server();
        let msg = server
            .entry_modify(Parameters(EntryModifyParams {
                entry: EntryRef::Path(path.to_path_buf()),
                title: Some(new_title.to_owned()),
                body: None,
                parent: UpdateOption::Unchanged,
                slug: None,
                tags: None,
                task_due: None,
                task_status: None,
                task_started_at: None,
                task_closed_at: None,
                event_start: None,
                event_end: None,
            }))
            .expect("entry_modify failed");
        assert!(
            msg.starts_with("updated"),
            "unexpected entry_modify response: {msg}"
        );
    }
}
