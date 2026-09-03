//! `serve::run` は、認証を通れる資格情報が 1 つも無いと bind すらせずに拒否する。
//!
//! 「認証を通れる」の意味は台帳の導入で変わった —— 鍵があるだけでは足りず、
//! その鍵が生きたデバイス行を指していなければならない。`harness::spawn()` は
//! `device add` を通すため、この不変条件はそこを経由せずに直接 `run` を呼んで
//! 確かめる。

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

/// 鍵はあるが、どのデバイスも指していない状態 —— 移行前の `gen-key` で作られた
/// 鍵ファイルをそのまま持ってきた場合がこれ。通る資格情報は 0 なので、鍵が
/// 0 本のときと同じく起動しない。
#[tokio::test]
async fn run_refuses_to_start_when_no_key_names_a_live_device() {
    let tmp = tempfile::tempdir().unwrap();
    let journal_dir = tmp.path().join("journal");
    sapphire_journal_core::init_app_context();
    harness::init_journal(&journal_dir);

    let keys_path = tmp.path().join("keys.toml");
    let mut keys = sapphire_framework::remote_server::KeyStore::load(&keys_path).unwrap();
    keys.generate(
        sapphire_journal_server::keys::TOKEN_PREFIX,
        None,
        None,
        Some("old".into()),
        None,
    )
    .unwrap();

    let journal_state = sapphire_journal_server::serve::open_journal_state(&journal_dir).unwrap();
    let state = sapphire_journal_server::serve::build_state(
        &journal_dir,
        &keys_path,
        Arc::clone(&journal_state),
    )
    .unwrap();

    let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    let result = sapphire_journal_server::serve::run(
        addr,
        &journal_dir,
        &keys_path,
        state,
        journal_state,
        &[],
    )
    .await;

    let message = result.unwrap_err().to_string();
    assert!(message.contains("no usable device key"), "{message}");
    assert!(message.contains("device add"), "逃げ道を示していない: {message}");
}

/// 対照。`device add` を通した鍵なら起動条件を満たす（bind の直前まで進む）。
#[tokio::test]
async fn run_gets_past_the_guard_with_a_device_key() {
    let tmp = tempfile::tempdir().unwrap();
    let journal_dir = tmp.path().join("journal");
    sapphire_journal_core::init_app_context();
    harness::init_journal(&journal_dir);
    let keys_path = tmp.path().join("keys.toml");
    harness::mint_device_key(&journal_dir, &keys_path, "laptop");

    let journal_state = sapphire_journal_server::serve::open_journal_state(&journal_dir).unwrap();
    let state = sapphire_journal_server::serve::build_state(
        &journal_dir,
        &keys_path,
        Arc::clone(&journal_state),
    )
    .unwrap();

    // すぐ止まるように、解決済みの shutdown を渡す。ガードを抜けられなければ
    // ここに到達する前にエラーで戻る。
    let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    let result = sapphire_journal_server::serve::run_until(
        addr,
        &journal_dir,
        &keys_path,
        state,
        journal_state,
        &[],
        std::future::ready(()),
    )
    .await;

    assert!(result.is_ok(), "生きたデバイスがあるのに起動できない: {result:?}");
}
