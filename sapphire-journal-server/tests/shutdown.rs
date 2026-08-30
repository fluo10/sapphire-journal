//! `serve::run` は shutdown 信号で待ち受けをやめて戻ること。
//!
//! 以前は `CancellationToken` を作って `with_graceful_shutdown` に渡すところ
//! まで書いてあったのに `cancel()` を呼ぶ者がどこにもおらず、graceful
//! shutdown は丸ごと死んだコードだった。systemd や `docker stop` のもとでは、
//! 書き込みの途中でプロセスが叩き切られていたことになる。
//!
//! ここが確かめるのは配線 —— 「shutdown が解決したら `run` が戻る」まで。
//! 本物の SIGTERM / Ctrl-C を受け取る部分（`serve::shutdown_signal`）は、
//! テストからシグナルを起こせないので外にある。

mod harness;

use std::sync::Arc;
use std::time::Duration;

#[tokio::test]
async fn run_returns_when_the_shutdown_future_resolves() {
    let tmp = tempfile::tempdir().unwrap();
    let journal_dir = tmp.path().join("journal");
    sapphire_journal_core::init_app_context();
    harness::init_journal(&journal_dir);

    // `run` は有効な鍵が 1 本も無いと bind 前に諦めるので、1 本発行しておく。
    // ここで諦めて戻られると、shutdown の配線を確かめたつもりで
    // 「鍵が無いから戻った」を見ることになる。
    let keys_path = tmp.path().join("keys.toml");
    let mut keys = sapphire_framework::remote_server::KeyStore::load(&keys_path).unwrap();
    keys.generate(sapphire_journal_server::keys::TOKEN_PREFIX, None, None, None, None)
        .unwrap();

    let journal_state = sapphire_journal_server::serve::open_journal_state(&journal_dir).unwrap();
    let state = sapphire_journal_server::serve::build_state(
        &journal_dir,
        &keys_path,
        Arc::clone(&journal_state),
    )
    .unwrap();

    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    let server = tokio::spawn(async move {
        sapphire_journal_server::serve::run_until(
            addr,
            &journal_dir,
            &keys_path,
            state,
            journal_state,
            &[],
            async move {
                let _ = rx.await;
            },
        )
        .await
    });

    // 送る前に落ちていないこと。ここが即座に終わっているなら、この後の
    // 「戻った」は shutdown のおかげではない。
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!server.is_finished(), "run() が shutdown を送る前に戻っている");

    tx.send(()).unwrap();

    let outcome = tokio::time::timeout(Duration::from_secs(20), server).await;
    let result = outcome.expect("run() が shutdown 信号のあとも戻ってこない（graceful shutdown が繋がっていない）");
    result.expect("run() の task が panic した").expect("run() がエラーで戻った");
}
