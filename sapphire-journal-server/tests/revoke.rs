//! 仕様が求めている鎖: `gen-key` → 両ルートが通る → `revoke-key` → 401。
//!
//! `KeyStore` は起動時に鍵ファイルを読んだスナップショットなので、失効が効くの
//! はサーバを組み直してからになる。だからこのテストも 2 段階で組み立てる ——
//! 「revoke してもプロセスを再起動するまで通り続ける」という**現在の仕様**を
//! 変えたら、ここが 1 段階目で落ちて気づける。
//!
//! 2 つの世代は journal を別々に持つ。鍵ファイルだけが共有物で、認証は journal
//! を一切見ない。同じ journal を 2 回開くと、まだ生きているほうの redb を掴んで
//! 「Database already open」になるだけで、確かめたい事柄とは無関係な失敗が
//! 混ざる。

mod harness;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use sapphire_journal_server::cli::Command;
use tower::ServiceExt as _;

async fn status(router: &axum::Router, uri: &str, token: &str) -> StatusCode {
    let body = match uri {
        "/rpc" => serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "workspace.snapshot", "params": {"ws": ""}
        }),
        _ => serde_json::json!({
            "jsonrpc": "2.0", "id": 0, "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18", "capabilities": {},
                "clientInfo": {"name": "revoke-test", "version": "0.0.0"}
            }
        }),
    };
    router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::ACCEPT, "application/json, text/event-stream")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::HOST, "localhost")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn a_revoked_key_gets_401_on_both_routes() {
    let tmp = tempfile::tempdir().unwrap();
    sapphire_journal_core::init_app_context();
    let journal_dir = tmp.path().join("before-revoke");
    let journal_after = tmp.path().join("after-revoke");
    harness::init_journal(&journal_dir);
    harness::init_journal(&journal_after);
    let keys_path = tmp.path().join("keys.toml");

    // ── gen-key ─────────────────────────────────────────────────────────────
    // 本物のサブコマンドを通す。トークンは stdout に出るので掴まえられない —
    // 鍵ファイルから読み直す（運用者が「なくしたら鍵ファイルを見ればいい」と
    // 案内されているのと同じ経路）。
    sapphire_journal_server::keys::run(
        Command::GenKey { label: Some("laptop".into()), expires_in: None },
        &keys_path,
    )
    .unwrap();
    let token = sapphire_framework::remote_server::KeyStore::load(&keys_path)
        .unwrap()
        .entries()[0]
        .token
        .clone();

    // ── 両ルートが通ること ──────────────────────────────────────────────────
    {
        let journal_state =
            sapphire_journal_server::serve::open_journal_state(&journal_dir).unwrap();
        let state = sapphire_journal_server::serve::build_state(
            &journal_dir,
            &keys_path,
            Arc::clone(&journal_state),
        )
        .unwrap();
        let router = sapphire_journal_server::serve::build_router(
            Arc::clone(&state),
            Arc::clone(&journal_state),
            tokio_util::sync::CancellationToken::new(),
            &[],
        )
        .unwrap();

        for uri in ["/rpc", "/mcp"] {
            assert_ne!(
                status(&router, uri, &token).await,
                StatusCode::UNAUTHORIZED,
                "{uri} が発行したばかりの鍵を拒んでいる（テストの前提が崩れている）"
            );
        }
    }

    // ── revoke-key ──────────────────────────────────────────────────────────
    sapphire_journal_server::keys::run(
        Command::RevokeKey { selector: "laptop".into() },
        &keys_path,
    )
    .unwrap();

    // 失効させたら残るのは 0 本。`serve::run` はこの状態では bind すら
    // しない（tests/no_keys.rs）ので、401 を確かめるには鍵を 1 本足して
    // サーバが起動できる状態にしておく必要がある —— 足した鍵が通ることが、
    // 401 の原因が「サーバが死んでいる」ではなく「その鍵が失効している」
    // ことの対照になる。
    sapphire_journal_server::keys::run(
        Command::GenKey { label: Some("replacement".into()), expires_in: None },
        &keys_path,
    )
    .unwrap();
    let replacement = sapphire_framework::remote_server::KeyStore::load(&keys_path)
        .unwrap()
        .entries()[0]
        .token
        .clone();
    assert_ne!(replacement, token);

    let journal_state = sapphire_journal_server::serve::open_journal_state(&journal_after).unwrap();
    let state = sapphire_journal_server::serve::build_state(
        &journal_after,
        &keys_path,
        Arc::clone(&journal_state),
    )
    .unwrap();
    let router = sapphire_journal_server::serve::build_router(
        Arc::clone(&state),
        Arc::clone(&journal_state),
        tokio_util::sync::CancellationToken::new(),
        &[],
    )
    .unwrap();

    for uri in ["/rpc", "/mcp"] {
        assert_eq!(
            status(&router, uri, &token).await,
            StatusCode::UNAUTHORIZED,
            "{uri} が失効した鍵をまだ受け付けている"
        );
        assert_ne!(
            status(&router, uri, &replacement).await,
            StatusCode::UNAUTHORIZED,
            "{uri} が生きている鍵まで拒んでいる（401 の原因が失効ではない）"
        );
    }

    // 鍵ファイルからも本当に消えていること。
    let store = sapphire_framework::remote_server::KeyStore::load(&keys_path).unwrap();
    assert!(
        store.entries().iter().all(|e| e.token != token),
        "失効させた鍵が鍵ファイルに残っている"
    );
}

