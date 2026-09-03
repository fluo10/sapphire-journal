//! 仕様が求めている鎖: `device add` → 両ルートが通る → `device retire` → 401。
//!
//! `KeyStore` も台帳も起動時に読んだスナップショットなので、引退が効くのは
//! サーバを組み直してからになる。だからこのテストも 2 段階で組み立てる ——
//! 「retire してもプロセスを再起動するまで通り続ける」という**現在の仕様**を
//! 変えたら、ここが 1 段階目で落ちて気づける。
//!
//! 2 つの世代は journal を別々に持つ。鍵ファイルだけが共有物。同じ journal を
//! 2 回開くと、まだ生きているほうの redb を掴んで「Database already open」に
//! なるだけで、確かめたい事柄とは無関係な失敗が混ざる。
//!
//! **台帳は 1 つ目の journal 側に置く。** `device retire` はそこを引く。

mod harness;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use sapphire_journal_server::cli_device::DeviceCommand;
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
                "clientInfo": {"name": "retire-test", "version": "0.0.0"}
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
async fn a_retired_device_gets_401_on_both_routes() {
    let tmp = tempfile::tempdir().unwrap();
    sapphire_journal_core::init_app_context();
    let journal_dir = tmp.path().join("before-retire");
    let journal_after = tmp.path().join("after-retire");
    harness::init_journal(&journal_dir);
    harness::init_journal(&journal_after);
    let keys_path = tmp.path().join("keys.toml");

    let token = harness::mint_device_key(&journal_dir, &keys_path, "laptop");

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
        // 台帳は 1 つ目の journal のもの。`device retire` もそこを引く。
        let device_auth = Arc::new(
            sapphire_journal_server::device_auth::DeviceAuth::load(
                &keys_path,
                &sapphire_journal_server::serve::default_devices_path(&journal_dir).unwrap(),
            )
            .unwrap(),
        );
        let router = sapphire_journal_server::serve::build_router(
            Arc::clone(&state),
            Arc::clone(&journal_state),
            tokio_util::sync::CancellationToken::new(),
            &[],
            device_auth,
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

    // ── device retire ───────────────────────────────────────────────────────
    sapphire_journal_server::cli_device::run_device(
        DeviceCommand::Retire { selector: "laptop".into(), purge: false },
        &sapphire_journal_server::serve::default_devices_path(&journal_dir).unwrap(),
        &sapphire_journal_server::serve::default_users_path(&journal_dir).unwrap(),
        &keys_path,
    )
    .unwrap();

    // 引退させたら鍵は 0 本。`serve::run` はこの状態では bind すらしない
    // （tests/no_keys.rs）ので、401 を確かめるには生きたデバイスを 1 つ
    // 足しておく必要がある —— それが通ることが、401 の原因が「サーバが
    // 死んでいる」ではなく「そのデバイスが引退した」ことの対照になる。
    let replacement = harness::mint_device_key(&journal_dir, &keys_path, "replacement");
    assert_ne!(replacement, token);

    let journal_state = sapphire_journal_server::serve::open_journal_state(&journal_after).unwrap();
    let state = sapphire_journal_server::serve::build_state(
        &journal_after,
        &keys_path,
        Arc::clone(&journal_state),
    )
    .unwrap();
    // 2 世代目も台帳は `journal_dir` 側（鍵ファイルと台帳は共有物で、
    // journal だけが世代ごとに別）。
    let device_auth = Arc::new(
        sapphire_journal_server::device_auth::DeviceAuth::load(
            &keys_path,
            &sapphire_journal_server::serve::default_devices_path(&journal_dir).unwrap(),
        )
        .unwrap(),
    );
    let router = sapphire_journal_server::serve::build_router(
        Arc::clone(&state),
        Arc::clone(&journal_state),
        tokio_util::sync::CancellationToken::new(),
        &[],
        device_auth,
    )
    .unwrap();

    for uri in ["/rpc", "/mcp"] {
        assert_eq!(
            status(&router, uri, &token).await,
            StatusCode::UNAUTHORIZED,
            "{uri} が引退したデバイスの鍵をまだ受け付けている"
        );
        assert_ne!(
            status(&router, uri, &replacement).await,
            StatusCode::UNAUTHORIZED,
            "{uri} が生きている鍵まで拒んでいる（401 の原因が引退ではない）"
        );
    }

    // 鍵ファイルからも本当に消えていること。
    let store = sapphire_framework::remote_server::KeyStore::load(&keys_path).unwrap();
    assert!(
        store.entries().iter().all(|e| e.token != token),
        "引退させたデバイスの鍵が鍵ファイルに残っている"
    );
}
