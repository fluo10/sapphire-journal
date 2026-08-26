//! `mcp_router` が単体で組み立てられ、`/mcp` に応答することを確かめる。

use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt as _;

#[tokio::test]
async fn mcp_router_serves_the_mcp_path() {
    let tmp = tempfile::tempdir().unwrap();
    sapphire_journal_core::init_app_context();
    let journal = init_journal(tmp.path());
    let state = Arc::new(Mutex::new(
        sapphire_journal_core::journal_state::JournalState::open(journal).unwrap(),
    ));

    let router = sapphire_journal_mcp::http::mcp_router(state, CancellationToken::new(), None);

    // GET /mcp は streamable-http では SSE ストリームの購読。405 でも 400 でもなく、
    // 「そのルートが存在する」ことだけをここでは確かめる。
    let response = router
        .oneshot(Request::builder().uri("/mcp").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_ne!(
        response.status(),
        StatusCode::NOT_FOUND,
        "/mcp が Router に載っていない"
    );
}

/// `.sapphire-journal/` を掘って journal にする。`ensure_journal` の init 分岐と同じ。
fn init_journal(root: &std::path::Path) -> sapphire_journal_core::journal::Journal {
    use sapphire_journal_core::journal::{Journal, JournalConfig};
    let journal_dir = root.join(".sapphire-journal");
    std::fs::create_dir_all(&journal_dir).unwrap();
    std::fs::write(
        journal_dir.join("config.toml"),
        toml::to_string_pretty(&JournalConfig::default()).unwrap(),
    )
    .unwrap();
    std::fs::write(journal_dir.join(".gitignore"), "cache/\n").unwrap();
    Journal::from_root(root.to_path_buf()).unwrap()
}
