//! `serve::run` は有効な鍵が 1 件も無いと bind すらせずに拒否する。
//!
//! `harness::spawn()` は鍵を 1 本発行してから状態を組み立てるため、この不変
//! 条件はそこを通さずに直接 `run` を呼んで確かめる。

mod harness;

use std::sync::Arc;

#[tokio::test]
async fn run_refuses_to_start_with_no_usable_key() {
    let tmp = tempfile::tempdir().unwrap();
    let journal_dir = tmp.path().join("journal");
    sapphire_journal_core::init_app_context();
    harness::init_journal(&journal_dir);

    // 存在すらしないファイル。`KeyStore::load` はファイルが無ければ空のストア
    // を返す(作成はしない)ので、これは「鍵が 0 本」の最も普通のケース。
    let keys_path = tmp.path().join("keys.toml");

    let journal_state = sapphire_journal_server::serve::open_journal_state(&journal_dir).unwrap();
    let state = sapphire_journal_server::serve::build_state(
        &journal_dir,
        &keys_path,
        Arc::clone(&journal_state),
    )
    .unwrap();

    let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    let result =
        sapphire_journal_server::serve::run(addr, &journal_dir, &keys_path, state, journal_state, &[])
            .await;

    assert!(
        result.is_err(),
        "run() should refuse to bind when no usable key is configured"
    );
    let message = result.unwrap_err().to_string();
    assert!(
        message.contains("no usable device key"),
        "error should name the actual problem, got: {message}"
    );
}
