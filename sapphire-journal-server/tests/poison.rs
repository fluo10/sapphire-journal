//! 一度の panic でサーバが恒久的に死なないこと。
//!
//! `JournalState` の Mutex は `/rpc` のリゾルバ・`/mcp` のツール・tick の
//! 3 つが共有している。`.lock().unwrap()` だと、そのどれか 1 か所で panic した
//! 瞬間から Mutex は毒され、以後**そのプロセスが生きている限り**残り全部が
//! panic し続ける —— しかも `ServerState::workspace()` は `workspaces` の
//! Mutex を握ったままリゾルバを呼ぶので、毒はそちらにも伝染して `/rpc` ごと
//! 落ちる。
//!
//! `JournalState` の不変条件は `sync()` が途中で止まっても壊れない（接続は
//! 呼び出しごとに開き直し、`sync_cache` はトランザクション）ので、毒は
//! 回収して構わない。

mod harness;

use std::sync::{Arc, Mutex};

use sapphire_journal_core::journal_state::JournalState;

/// `journal_state` を毒す。ロックを握ったまま panic したスレッドを 1 つ作る。
fn poison(state: &Arc<Mutex<JournalState>>) {
    let poisoner = Arc::clone(state);
    let _ = std::thread::spawn(move || {
        let _guard = poisoner.lock().unwrap();
        panic!("a tool panicked while holding the journal lock");
    })
    .join();
    assert!(state.is_poisoned(), "テストの前提: Mutex が毒されているはず");
}

struct Fixture {
    _tmp: tempfile::TempDir,
    state: Arc<sapphire_framework::remote_server::ServerState>,
    journal_state: Arc<Mutex<JournalState>>,
    ws: String,
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let journal_dir = tmp.path().join("journal");
    sapphire_journal_core::init_app_context();
    harness::init_journal(&journal_dir);

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
    let ws = sapphire_journal_server::serve::workspace_id(&journal_dir).unwrap();

    Fixture { _tmp: tmp, state, journal_state, ws }
}

#[test]
fn the_workspace_resolver_survives_a_poisoned_journal_lock() {
    let f = fixture();
    // `workspace()` はワークスペースごとにメモ化するので、毒す前に一度も
    // 呼んでいないこと自体がこのテストの前提 —— 呼んでいたらリゾルバは
    // 二度と走らず、毒とすれ違ってしまう。
    poison(&f.journal_state);

    let store = f.state.workspace(&f.ws);

    assert!(
        store.is_ok(),
        "毒された Mutex のせいで /rpc のワークスペース解決が失敗した: {:?}",
        store.err()
    );
}

#[test]
fn the_tick_survives_a_poisoned_journal_lock() {
    let f = fixture();
    let store = f.state.workspace(&f.ws).unwrap();
    poison(&f.journal_state);

    let report = sapphire_journal_server::serve::tick_once(&store, &f.journal_state);

    assert!(report.is_ok(), "毒された Mutex のせいで tick が失敗した: {:?}", report.err());
}
