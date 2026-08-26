//! `/mcp` は rmcp の `Host` 許可リストの後ろにいる。既定はループバックだけ
//! なので、`--addr 0.0.0.0:8080` で待ち受けても、`Host: box.tailnet.ts.net`
//! で来たクライアントは 403 になる —— `/rpc` にはその検査が無いため「同期は
//! 動いているのに MCP だけ死んでいる」という片肺の壊れ方をする。
//!
//! ここで確かめるのはその 2 面：許可していないホスト名は今も弾かれること
//! （検査自体を無効化していないこと）と、`--allowed-host` で名指ししたホスト名
//! なら**ツール呼び出しまで届く**こと。

mod harness;

use axum::http::StatusCode;

const REMOTE_HOST: &str = "box.tailnet.ts.net";

#[tokio::test]
async fn a_non_loopback_host_is_rejected_when_it_was_not_allowed() {
    // 許可リストを広げていないサーバ（= 既定）。
    let h = harness::spawn().await;

    let (status, body) = h.mcp_initialize_status_with_host(REMOTE_HOST).await;

    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "許可していないホスト名が通ってしまった（DNS リバインディング対策が \
         効いていない）: {body}"
    );
}

#[tokio::test]
async fn the_two_routes_disagree_about_an_unlisted_host() {
    // これがこの不具合の見つけにくさそのもの。同じトークン、同じホスト名で
    // `/rpc` は 200、`/mcp` は 403。運用者から見ると「同期は動いている」ので
    // サーバは生きているように見える。
    let h = harness::spawn().await;

    let rpc = h.rpc_status_with_host(REMOTE_HOST).await;
    let (mcp, _) = h.mcp_initialize_status_with_host(REMOTE_HOST).await;

    assert_eq!(rpc, StatusCode::OK, "/rpc は Host を見ない想定");
    assert_eq!(mcp, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_named_host_reaches_an_mcp_tool() {
    // `--allowed-host box.tailnet.ts.net` を渡した状態。ハンドシェイクから
    // `tools/call` まで、すべてこのホスト名で送られる（harness の `mcp_host`）
    // ので、許可リストが繋がっていなければ `spawn_with_allowed_hosts` の中の
    // `initialize` が 403 で落ちてこのテストは通らない。
    let h = harness::spawn_with_allowed_hosts(&[REMOTE_HOST.to_owned()], REMOTE_HOST).await;

    let path = h.write_entry_through_mcp("from a remote client").await;

    assert!(
        path.exists(),
        "ツールは 200 を返したのにエントリが書かれていない: {path:?}"
    );
    // ツールが本当に走ったことを、返ってきた文字列だけでなくファイルの中身で
    // 確かめる。
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("from a remote client"), "{text}");
}

#[tokio::test]
async fn widening_the_list_keeps_loopback_working() {
    // 許可リストは「置き換え」ではなく「追加」。リモート名を足したせいで
    // ローカルの MCP クライアントが 403 になる、では直したことにならない。
    let h = harness::spawn_with_allowed_hosts(&[REMOTE_HOST.to_owned()], "localhost").await;

    let (status, body) = h.mcp_initialize_status_with_host("127.0.0.1").await;

    assert_eq!(status, StatusCode::OK, "{body}");
}
