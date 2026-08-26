//! サーバの組み立てと起動。

use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::Context as _;
use axum::Router;
use sapphire_framework::remote_server::{KeyStore, ServerState, WsStoreConfig, protect, router};
use sapphire_journal_core::journal::Journal;
use sapphire_journal_core::journal_state::JournalState;
use tokio_util::sync::CancellationToken;

/// このサーバが公開するワークスペースの識別子。
///
/// journal の UUID をそのまま使う。名前ではなく ID にしてあるのは、クライアント
/// 側の設定がディレクトリ名の変更で壊れないようにするため。
pub fn workspace_id(journal_dir: &Path) -> anyhow::Result<String> {
    let journal = Journal::from_root(journal_dir.to_path_buf())?;
    Ok(journal.journal_id()?.to_string())
}

/// 鍵ファイルの既定位置。journal ルートの**外**（cache dir）に置く — origin の
/// 中に置くと同期でクライアントに配られる。
pub fn default_keys_path(journal_dir: &Path) -> anyhow::Result<std::path::PathBuf> {
    let journal = Journal::from_root(journal_dir.to_path_buf())?;
    Ok(journal.cache_dir()?.join("keys.toml"))
}

/// journal のキャッシュを開く。MCP と同期の両方がこの 1 つを共有する。
pub fn open_journal_state(journal_dir: &Path) -> anyhow::Result<Arc<Mutex<JournalState>>> {
    let journal = Journal::from_root(journal_dir.to_path_buf())?;
    let state = JournalState::open(journal)?;
    state.sync()?;
    Ok(Arc::new(Mutex::new(state)))
}

/// framework のサーバ状態を組み立てる。
///
/// resolver は journal ルートを origin に、journal 自身の retrieve ストアを
/// 注入した [`WsStoreConfig`] を返す。同じストアを使わないと、同じファイル群に
/// インデックスが 2 つできる。
pub fn build_state(
    journal_dir: &Path,
    keys_path: &Path,
    journal_state: Arc<Mutex<JournalState>>,
) -> anyhow::Result<Arc<ServerState>> {
    let keys = KeyStore::load(keys_path)
        .with_context(|| format!("loading API keys from {}", keys_path.display()))?;

    let journal = Journal::from_root(journal_dir.to_path_buf())?;
    let expected_ws = journal.journal_id()?.to_string();
    let origin_dir = journal.root.clone();
    let state_dir = journal.cache_dir()?.join("server");

    let resolver_state = Arc::clone(&journal_state);
    let state = ServerState::new(&state_dir)
        .with_keys(Arc::new(keys))
        .with_resolver(move |ws| {
            if ws != expected_ws {
                // このサーバは 1 つの journal しか公開しない。名前で拒否しないと、
                // 適当な名前を送られるたびに同じ origin に対する WsStore が別
                // インスタンスとして生まれ、change log が複数本になる。
                return Err(sapphire_framework::remote_server::Error::UnknownWorkspace(
                    ws.to_owned(),
                ));
            }
            let retrieve = resolver_state.lock().unwrap().retrieve_db().shared();
            Ok(WsStoreConfig {
                origin_dir: origin_dir.clone(),
                state_dir: state_dir.clone(),
                retrieve: Some(retrieve),
                app_dir: Some(".sapphire-journal".to_owned()),
            })
        });

    Ok(Arc::new(state))
}

/// `/rpc` と `/mcp` を 1 つの Router に束ね、両方に同じ鍵をかける。
pub fn build_router(
    state: Arc<ServerState>,
    journal_state: Arc<Mutex<JournalState>>,
    cancel: CancellationToken,
) -> anyhow::Result<Router> {
    // オブザーバは Task 7 で繋ぐ。ここでは None。
    let mcp = sapphire_journal_mcp::http::mcp_router(journal_state, cancel, None);
    // `router()` は認証適用済み。`/mcp` は自分で protect に通す — 片方だけ
    // 守られている状態を作らない。
    Ok(router(Arc::clone(&state)).merge(protect(state, mcp)))
}

/// listener を開いて待ち受ける。
pub async fn run(
    addr: std::net::SocketAddr,
    journal_dir: &Path,
    keys_path: &Path,
    state: Arc<ServerState>,
    journal_state: Arc<Mutex<JournalState>>,
) -> anyhow::Result<()> {
    let cancel = CancellationToken::new();
    let app = build_router(Arc::clone(&state), Arc::clone(&journal_state), cancel.clone())?;

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind {addr}"))?;

    tracing::info!(
        %addr,
        journal = %journal_dir.display(),
        keys = %keys_path.display(),
        "sapphire-journal-server listening on /rpc and /mcp — private network only: \
         no TLS, no OAuth, tokens are stored in plaintext"
    );

    // 定期ティックは Task 8 でここに足す。
    axum::serve(listener, app)
        .with_graceful_shutdown(async move { cancel.cancelled().await })
        .await
        .context("server failed")
}
