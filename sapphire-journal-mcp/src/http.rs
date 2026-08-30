//! HTTP transport for the MCP server: [`mcp_router`] builds the `/mcp` route,
//! [`serve_http`] binds that route to a socket of its own.
//!
//! A host that already runs an `axum` server takes the router and merges it
//! into its own — `sapphire-journal-server` does that, so the sync API and
//! MCP share one process, one port and one auth layer. A host that wants MCP
//! and nothing else (the desktop frontend) calls `serve_http`.
//!
//! Both speak the [MCP Streamable HTTP transport][spec] under `/mcp`.
//!
//! ## The `Host` allowlist is not optional
//!
//! rmcp defends against DNS rebinding by refusing any request whose `Host`
//! header is not on an allowlist, and its default list is loopback only
//! (`localhost`, `127.0.0.1`, `::1`). That default is correct for a router
//! bound to loopback and **wrong the moment the host binds anywhere else**: a
//! client reaching the server as `http://10.0.0.5:8080/mcp` or
//! `http://box.tailnet.ts.net/mcp` sends a `Host` that matches nothing and
//! gets `403 Forbidden`. A host that widens its bind address without widening
//! this list fails *partially*, and quietly: a sibling route with no such
//! guard (`/rpc`) keeps answering, so sync looks healthy while MCP is dead.
//!
//! [`mcp_router`] therefore takes the extra hostnames as an argument instead
//! of leaving them at a default nobody remembers to change. Loopback is always
//! added on top of what the caller passes, so widening the list never costs
//! local use, and passing nothing can never *disable* the guard — an empty
//! list means "allow every host" to rmcp, and this module never hands it one.
//!
//! Authentication is a separate concern this module does not provide; see
//! [`mcp_router`].
//!
//! [spec]: https://modelcontextprotocol.io/specification/2025-06-18/basic/transports#streamable-http

use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::sync::Arc;

use anyhow::Context as _;
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
};
use tokio_util::sync::CancellationToken;

use sapphire_journal_core::JournalState;

use crate::server::{prepare_state, spawn_periodic_reindex, SapphireJournalServer};

/// rmcp 既定の `Host` 許可リスト。ループバックのみ。
///
/// [`mcp_router`] は呼び出し側が渡したホスト名をこの**上に足す**。置き換えな
/// いのは、bind を広げたホストでもローカルからの接続を壊さないため。
const LOOPBACK_HOSTS: [&str; 3] = ["localhost", "127.0.0.1", "::1"];

/// MCP を `/mcp` に載せた [`axum::Router`] を返す。
///
/// `allowed_hosts` には**クライアントが実際に使うホスト名**を渡す —— Tailscale
/// の名前、LAN のホスト名、bind したアドレス。ループバックは常に足されるので
/// ここは追加分だけでよく、空スライスを渡せば rmcp 既定と同じループバック限定
/// になる（rmcp が「空リスト = 全ホスト許可」と解釈する状態には決してならない）。
/// モジュールの解説の通り、bind アドレスだけ広げてここを広げないと `/mcp` だけ
/// が 403 で死ぬ。
///
/// 認証は**かけない**。呼び出し側が framework の `protect()` などで包むこと
/// （`sapphire-journal-server` がそうしている）。単体で外に晒すとこの `Host`
/// 許可リストしか守るものが無く、それは DNS リバインディング対策であって認証
/// ではない。
pub fn mcp_router(
    shared_state: Arc<std::sync::Mutex<JournalState>>,
    cancel: CancellationToken,
    observer: Option<crate::server::WriteObserver>,
    allowed_hosts: &[String],
) -> axum::Router {
    let factory_state = Arc::clone(&shared_state);
    let factory = move || {
        let server = SapphireJournalServer::from_shared(Arc::clone(&factory_state));
        // セッションごとに新しいサーバが作られるので、オブザーバは毎回付け直す。
        Ok(match &observer {
            Some(o) => server.with_write_observer(Arc::clone(o)),
            None => server,
        })
    };

    let hosts: Vec<String> = LOOPBACK_HOSTS
        .iter()
        .map(|h| (*h).to_owned())
        .chain(
            allowed_hosts
                .iter()
                .filter(|h| !h.trim().is_empty())
                .cloned(),
        )
        .collect();
    tracing::debug!(?hosts, "MCP Host allowlist");
    let config = StreamableHttpServerConfig::default()
        .with_cancellation_token(cancel)
        .with_allowed_hosts(hosts);
    let http_service = StreamableHttpService::new(
        factory,
        Arc::new(LocalSessionManager::default()),
        config,
    );

    axum::Router::new().route_service("/mcp", http_service)
}

/// Bind an HTTP MCP server to `bind:port`, serving the journal at
/// `journal_dir`. Runs until `cancel` is triggered, at which point active
/// connections are gracefully drained and the periodic re-index task is
/// aborted.
///
/// `journal_dir` is opened directly; the upward-search fallback used by the
/// CLI/stdio path is intentionally disabled here because GUI hosts know
/// exactly which journal they want to expose.
pub async fn serve_http(
    journal_dir: &Path,
    bind: &str,
    port: u16,
    cancel: CancellationToken,
) -> anyhow::Result<()> {
    let ip: IpAddr = bind
        .parse()
        .with_context(|| format!("invalid MCP HTTP bind address: {bind}"))?;
    let addr = SocketAddr::from((ip, port));

    let state = prepare_state(Some(journal_dir), false)?;
    let shared_state = Arc::new(std::sync::Mutex::new(state));
    // ループバック限定のまま。`serve_http` は GUI ホストが自分のマシンで使う
    // 入口で、`bind` を広げるときにホスト名を渡す口はまだ無い。
    let router = mcp_router(Arc::clone(&shared_state), cancel.clone(), None, &[]);

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind MCP HTTP server to {addr}"))?;
    tracing::info!("MCP HTTP server listening on http://{addr}/mcp");

    let sync_handle = spawn_periodic_reindex(shared_state);

    let serve_result = axum::serve(listener, router)
        .with_graceful_shutdown(async move { cancel.cancelled().await })
        .await;

    if let Some(handle) = sync_handle {
        handle.abort();
    }

    serve_result.context("MCP HTTP server failed")
}
