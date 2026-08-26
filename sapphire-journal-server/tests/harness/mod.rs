//! テスト用に journal を 1 つ作り、鍵を 1 本発行してサーバを組み立てる。

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Body;
use axum::http::{Request, header};
use http_body_util::BodyExt as _;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt as _;

pub struct Harness {
    _tmp: tempfile::TempDir,
    pub journal_dir: PathBuf,
    pub token: String,
    pub ws: String,
    router: Router,
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
        CancellationToken::new(),
    )
    .unwrap();

    Harness { _tmp: tmp, journal_dir, token, ws, router }
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
}
