//! HTTP transport for the MCP server.
//!
//! Used by long-running host processes (typically the desktop frontend) that
//! want agents to connect to an already-running journal session over HTTP
//! instead of spawning a stdio subprocess.
//!
//! The server speaks the [MCP Streamable HTTP transport][spec] under `/mcp`.
//! By default rmcp restricts the `Host` header to loopback addresses to
//! prevent DNS rebinding attacks; do NOT bind to `0.0.0.0` without also
//! widening `allowed_hosts` and adding an authentication layer.
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

use crate::server::{prepare_state, spawn_periodic_git_sync, ArchelonServer};

/// Bind an HTTP MCP server to `bind:port`, serving the journal at
/// `journal_dir`. Runs until `cancel` is triggered, at which point active
/// connections are gracefully drained and the periodic git-sync task is
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

    let factory_state = Arc::clone(&shared_state);
    let factory = move || Ok(ArchelonServer::from_shared(Arc::clone(&factory_state)));

    let config = StreamableHttpServerConfig::default().with_cancellation_token(cancel.clone());
    let http_service = StreamableHttpService::new(
        factory,
        Arc::new(LocalSessionManager::default()),
        config,
    );

    let router = axum::Router::new().route_service("/mcp", http_service);

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind MCP HTTP server to {addr}"))?;
    tracing::info!("MCP HTTP server listening on http://{addr}/mcp");

    let sync_handle = spawn_periodic_git_sync(shared_state);

    let serve_result = axum::serve(listener, router)
        .with_graceful_shutdown(async move { cancel.cancelled().await })
        .await;

    if let Some(handle) = sync_handle {
        handle.abort();
    }

    serve_result.context("MCP HTTP server failed")
}
