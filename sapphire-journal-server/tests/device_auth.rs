//! fail-closed の各条件が、`/rpc` と `/mcp` の両方で 401 になること。
//!
//! `DeviceAuth` の単体テストは解決だけを見る。ここが見るのは「そのレイヤが
//! 本当に両方のルートに被さっているか」—— 片方だけ守られている状態は、
//! 解決の正しさとは独立に起こりうる。

mod harness;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use sapphire_framework::remote_server::KeyStore;
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
                "clientInfo": {"name": "device-auth-test", "version": "0.0.0"}
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

/// journal と台帳と鍵を 1 式作り、Router を組む。`extra` は、組む前に鍵や
/// 台帳へ細工をするフック。その戻り値（細工で作ったトークンなど）はそのまま
/// 返す —— 呼び出し側で `&mut` を閉じ込めずに済ませるため。
async fn router_with<T>(
    tmp: &tempfile::TempDir,
    extra: impl FnOnce(&std::path::Path, &std::path::Path) -> T,
) -> (axum::Router, String, T) {
    sapphire_journal_core::init_app_context();
    let journal_dir = tmp.path().join("journal");
    harness::init_journal(&journal_dir);
    let keys_path = tmp.path().join("keys.toml");
    let live = harness::mint_device_key(&journal_dir, &keys_path, "live");
    let devices_path =
        sapphire_journal_server::serve::default_devices_path(&journal_dir).unwrap();

    let extra_out = extra(&keys_path, &devices_path);

    let journal_state = sapphire_journal_server::serve::open_journal_state(&journal_dir).unwrap();
    let state = sapphire_journal_server::serve::build_state(
        &journal_dir,
        &keys_path,
        Arc::clone(&journal_state),
    )
    .unwrap();
    let device_auth = Arc::new(
        sapphire_journal_server::device_auth::DeviceAuth::load(&keys_path, &devices_path).unwrap(),
    );
    let router = sapphire_journal_server::serve::build_router(
        state,
        journal_state,
        tokio_util::sync::CancellationToken::new(),
        &[],
        device_auth,
    )
    .unwrap();
    (router, live, extra_out)
}

#[tokio::test]
async fn a_device_key_passes_and_a_key_without_a_device_does_not() {
    let tmp = tempfile::tempdir().unwrap();
    let (router, live, orphan) = router_with(&tmp, |keys_path, _devices| {
        // 移行前の `gen-key` が作っていた形の鍵。
        let mut keys = KeyStore::load(keys_path).unwrap();
        keys.generate(
            sapphire_journal_server::keys::TOKEN_PREFIX,
            None,
            None,
            Some("old".into()),
            None,
        )
        .unwrap()
        .token
    })
    .await;

    for uri in ["/rpc", "/mcp"] {
        assert_ne!(
            status(&router, uri, &live).await,
            StatusCode::UNAUTHORIZED,
            "{uri} が生きたデバイスを拒んでいる（テストの前提が崩れている）"
        );
        assert_eq!(
            status(&router, uri, &orphan).await,
            StatusCode::UNAUTHORIZED,
            "{uri} が台帳を経由しない鍵を通している"
        );
    }
}

#[tokio::test]
async fn a_retired_device_gets_401_even_while_its_key_is_still_in_the_file() {
    let tmp = tempfile::tempdir().unwrap();
    // `device retire` は鍵も失効させるので、この状態は手編集か同期でしか
    // 起きない。それでも通してはいけない —— 認証の判断は台帳が持つ。
    let (router, live, ()) = router_with(&tmp, |_keys, devices_path| {
        sapphire_framework::registry::Devices::load(devices_path)
            .unwrap()
            .retire("live")
            .unwrap();
    })
    .await;

    for uri in ["/rpc", "/mcp"] {
        assert_eq!(
            status(&router, uri, &live).await,
            StatusCode::UNAUTHORIZED,
            "{uri} が引退したデバイスを通している"
        );
    }
}

#[tokio::test]
async fn a_key_naming_a_missing_row_gets_401() {
    let tmp = tempfile::tempdir().unwrap();
    let (router, live, ()) = router_with(&tmp, |_keys, devices_path| {
        sapphire_framework::registry::Devices::load(devices_path)
            .unwrap()
            .purge("live")
            .unwrap();
    })
    .await;

    for uri in ["/rpc", "/mcp"] {
        assert_eq!(
            status(&router, uri, &live).await,
            StatusCode::UNAUTHORIZED,
            "{uri} が台帳に無いデバイスを通している"
        );
    }
}

#[tokio::test]
async fn no_bearer_header_gets_401() {
    let tmp = tempfile::tempdir().unwrap();
    let (router, _live, ()) = router_with(&tmp, |_keys, _devices| {}).await;

    for uri in ["/rpc", "/mcp"] {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::ACCEPT, "application/json, text/event-stream")
                    .header(header::HOST, "localhost")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{uri}");
    }
}
