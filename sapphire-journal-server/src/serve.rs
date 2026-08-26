//! サーバの組み立てと起動。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Context as _;
use axum::Router;
use sapphire_framework::remote_server::{
    KeyStore, ReconcileReport, ServerState, WsStore, WsStoreConfig, protect, router,
};
use sapphire_journal_core::journal::Journal;
use sapphire_journal_core::journal_state::JournalState;
use sapphire_journal_core::user_config::UserConfig;
use tokio_util::sync::CancellationToken;

/// `UserConfig::sync_interval()` が使えない(設定が読めない)ときの既定間隔。
///
/// **`sync_interval_minutes = 0` は意図的にここでは無効を意味しない。**
/// `UserConfig` 自身のドキュメントは「0 で無効化」と書いてあり、MCP 側の
/// `spawn_periodic_reindex` はその通り `None` を返して自分自身を起動しない
/// —— クライアントではそれで正しい。書き込み通知の経路と再インデックスが
/// 二重になるだけだから。
///
/// だがこのサーバでは `spawn_periodic_reindex` を最初から一切呼んでおらず、
/// この tick が唯一の整合性の網（Task 7 のオブザーバの取りこぼし、手作業、
/// 外部ツールの編集を拾う場所）になる。無関係な理由で誰かが 0 を設定した
/// せいでこの安全網まで黙って止まるのは、0 を無視するより悪い。将来ここを
/// 「MCP と挙動を揃える」方向に「直さない」こと —— `run` 側で 0 を検出したら
/// 警告ログを出したうえでこの既定値にフォールバックする、まで込みで意図的。
const DEFAULT_TICK_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// このサーバが公開するワークスペースの識別子。
///
/// journal の UUID をそのまま使う。名前ではなく ID にしてあるのは、クライアント
/// 側の設定がディレクトリ名の変更で壊れないようにするため。
pub fn workspace_id(journal_dir: &Path) -> anyhow::Result<String> {
    let journal = Journal::from_root(journal_dir.to_path_buf())?;
    Ok(journal.journal_id()?.to_string())
}

/// 鍵ファイルの既定位置。journal ルートの**外**（cache dir）に置く — origin の
/// 中に置くと同期でクライアントに配られる。
pub fn default_keys_path(journal_dir: &Path) -> anyhow::Result<std::path::PathBuf> {
    let journal = Journal::from_root(journal_dir.to_path_buf())?;
    Ok(journal.cache_dir()?.join("keys.toml"))
}

/// journal のキャッシュを開く。MCP と同期の両方がこの 1 つを共有する。
pub fn open_journal_state(journal_dir: &Path) -> anyhow::Result<Arc<Mutex<JournalState>>> {
    let journal = Journal::from_root(journal_dir.to_path_buf())?;
    let state = JournalState::open(journal)?;
    state.sync()?;
    Ok(Arc::new(Mutex::new(state)))
}

/// framework のサーバ状態を組み立てる。
///
/// resolver は journal ルートを origin に、journal 自身の retrieve ストアを
/// 注入した [`WsStoreConfig`] を返す。同じストアを使わないと、同じファイル群に
/// インデックスが 2 つできる。
pub fn build_state(
    journal_dir: &Path,
    keys_path: &Path,
    journal_state: Arc<Mutex<JournalState>>,
) -> anyhow::Result<Arc<ServerState>> {
    let keys = KeyStore::load(keys_path)
        .with_context(|| format!("loading API keys from {}", keys_path.display()))?;

    let journal = Journal::from_root(journal_dir.to_path_buf())?;
    let expected_ws = journal.journal_id()?.to_string();
    let origin_dir = journal.root.clone();
    let state_dir = journal.cache_dir()?.join("server");

    let resolver_state = Arc::clone(&journal_state);
    let state = ServerState::new(&state_dir)
        .with_keys(Arc::new(keys))
        .with_resolver(move |ws| {
            if ws != expected_ws {
                // このサーバは 1 つの journal しか公開しない。名前で拒否しないと、
                // 適当な名前を送られるたびに同じ origin に対する WsStore が別
                // インスタンスとして生まれ、change log が複数本になる。
                return Err(sapphire_framework::remote_server::Error::UnknownWorkspace(
                    ws.to_owned(),
                ));
            }
            let retrieve = resolver_state.lock().unwrap().retrieve_db().shared();
            Ok(WsStoreConfig {
                origin_dir: origin_dir.clone(),
                state_dir: state_dir.clone(),
                retrieve: Some(retrieve),
                app_dir: Some(".sapphire-journal".to_owned()),
            })
        });

    Ok(Arc::new(state))
}

/// bind アドレスと `--allowed-host` から、MCP に渡す `Host` 許可リストを作る。
///
/// rmcp は `Host` ヘッダを許可リストで検査する（DNS リバインディング対策）。
/// `--addr 0.0.0.0:8080` で待ち受けても、クライアントが送ってくる `Host` は
/// `0.0.0.0` ではなく **そのクライアントが URL に書いた名前** —— `10.0.0.5:8080`
/// だったり `box.tailnet.ts.net` だったり —— なので、bind アドレスから機械的に
/// 導けるのはせいぜい「同じマシンから bind アドレスを直接叩く」場合だけ。残りは
/// 運用者が `--allowed-host` で名指しするしかない。だから両方を足す。
///
/// ループバックは [`sapphire_journal_mcp::http::mcp_router`] が常に足すので
/// ここには入れない。
pub fn allowed_hosts(addr: std::net::SocketAddr, extra: &[String]) -> Vec<String> {
    let mut hosts = vec![
        // ホスト単体（ポート無し）は rmcp では任意ポートに一致し、`host:port`
        // は完全一致。両方入れておくと、リバースプロキシがポートを書き換えた
        // 場合も、書き換えない場合も通る。
        addr.ip().to_string(),
        addr.to_string(),
    ];
    hosts.extend(
        extra
            .iter()
            .map(|h| h.trim())
            .filter(|h| !h.is_empty())
            .map(str::to_owned),
    );
    hosts.dedup();
    hosts
}

/// `/rpc` と `/mcp` を 1 つの Router に束ね、両方に同じ鍵をかける。
///
/// `allowed_hosts` は MCP の `Host` 許可リストの追加分（[`allowed_hosts`]
/// が組み立てる）。`/rpc` 側にこの検査は無いので、ここを空のままにすると
/// 「同期は通るが `/mcp` だけ 403」という片肺状態になる。
///
/// MCP の書き込みを change log に載せるオブザーバもここで繋ぐ — `mcp_router`
/// がセッションごとにファクトリで `SapphireJournalServer` を作る以上、外から
/// 後付けする経路は無い。
pub fn build_router(
    state: Arc<ServerState>,
    journal_state: Arc<Mutex<JournalState>>,
    journal_dir: &Path,
    cancel: CancellationToken,
    allowed_hosts: &[String],
) -> anyhow::Result<Router> {
    let ws = workspace_id(journal_dir)?;
    // /rpc と同じインスタンス。ここで with_config を自分で呼ぶと change log が 2 本になる。
    let store = state.workspace(&ws)?;
    let origin = Journal::from_root(journal_dir.to_path_buf())?.root;
    let observer = write_observer(store, origin);

    let mcp =
        sapphire_journal_mcp::http::mcp_router(journal_state, cancel, Some(observer), allowed_hosts);
    // `router()` は認証適用済み。`/mcp` は自分で protect に通す — 片方だけ
    // 守られている状態を作らない。
    Ok(router(Arc::clone(&state)).merge(protect(state, mcp)))
}

/// MCP が書いたパスを change log に載せる。
///
/// パスは絶対で来るので origin 相対の POSIX に直す。`record_local_write` は
/// 1 回の呼び出しが 1 バッチなので、リネームの旧・新はここで 1 回にまとまる
/// （通知側が 1 回で渡してくる）。
///
/// **`origin` は MCP 側が journal ルートに使っているのと綴りが完全に一致して
/// いること。** `strip_prefix` は純粋な文字列比較で、`Journal::from_root` は
/// 正規化（大文字小文字の統一やシンボリックリンク解決など）を一切しない
/// ため、綴りが少しでもずれるとそのパスは黙って「origin の外」判定され、
/// 書き込みが change log に載らない。
fn write_observer(store: Arc<WsStore>, origin: PathBuf) -> sapphire_journal_mcp::server::WriteObserver {
    Arc::new(move |paths: &[PathBuf]| {
        let rel: Vec<String> = paths
            .iter()
            .filter_map(|p| match p.strip_prefix(&origin) {
                Ok(rel) => Some(rel),
                Err(_) => {
                    // 黙って捨てない。origin の綴りがずれている、あるいは
                    // ツールが journal の外に書いた徴候 — どちらも見えないと
                    // 原因調査ができない。
                    tracing::warn!(
                        path = %p.display(),
                        origin = %origin.display(),
                        "MCP write path is outside origin; dropping it from the change log"
                    );
                    None
                }
            })
            .map(|p| {
                p.components()
                    .map(|c| c.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/")
            })
            .collect();
        if rel.is_empty() {
            tracing::warn!(
                paths = ?paths,
                "MCP write batch had no recordable path; nothing sent to the change log"
            );
            return;
        }
        if let Err(e) = store.record_local_write(&rel, chrono::Utc::now()) {
            // 落とさない。取りこぼしは Task 8 の定期 reconcile が回収する。
            tracing::warn!(error = %e, paths = ?rel, "failed to record an MCP write");
        }
    })
}

/// 1 回分の整合処理。journal のキャッシュ同期と change log の追随を、
/// この順で行う。
///
/// **サーバ構成では走査はここだけ。** MCP の定期再インデックス
/// (`spawn_periodic_reindex`) は呼ばない — 同じファイル群を 2 回舐めるだけで、
/// しかも片方は change log を更新しない。
pub fn tick_once(
    store: &WsStore,
    journal_state: &Mutex<JournalState>,
) -> anyhow::Result<ReconcileReport> {
    if journal_state.is_poisoned() {
        // 通常の失敗ではない。過去のどこかのティックが journal_state を
        // 握ったまま panic して以来、この Mutex は poisoned のまま —
        // つまり直後の `lock().unwrap()` は必ず panic し、この先も
        // プロセスを再起動するまで毎ティック同じことが起きる
        // (spawn_blocking が拾って `spawn_tick` 側で「panicked」と
        // ログには出るが、それだけでは「たまたま今回失敗した」のか
        // 「もう死んでいて二度と動かない」のか区別できない)。ロック方針
        // 自体は変えない — ここでは何が起きているかを一度はっきり
        // 名指しするだけ。
        tracing::error!(
            "journal_state mutex is poisoned by an earlier panic; every tick will keep \
             panicking here until the process restarts"
        );
    }
    journal_state.lock().unwrap().sync()?;
    let report = store.reconcile()?;

    // reconcile が回収した後、パス単位 LWW が旧パスを復活させて同じ id が
    // 2 つのファイルに分かれていないか確認する（Task 9）。
    //
    // `sync()` を先に呼ぶ順序は変えないこと。両方が entry として読める重複は
    // `sync_cache` の `increment_until_free` が id を振り直して両方残すので、
    // `resolve_duplicates` には届かない —— 届くのは片方が読めない場合だけで、
    // そのときの残す/退避するの判断も、この `sync()` が更新したキャッシュの
    // id → パス対応を根拠にしている。
    let guard = journal_state.lock().unwrap();
    let quarantined = crate::dedupe::resolve_duplicates(store, &guard)?;
    drop(guard);
    if quarantined > 0 {
        tracing::info!(
            quarantined,
            "resolved files sharing an entry id; the losers were moved to the journal's \
             cache dir under quarantine/, not deleted"
        );
    }

    Ok(report)
}

/// [`tick_once`] を `interval` ごとに回す。起動直後に 1 回走らせてから待つ。
pub fn spawn_tick(
    store: Arc<WsStore>,
    journal_state: Arc<Mutex<JournalState>>,
    interval: Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        loop {
            ticker.tick().await;
            let store = Arc::clone(&store);
            let journal_state = Arc::clone(&journal_state);
            // redb / tantivy / ファイル走査はブロッキング。
            let result = tokio::task::spawn_blocking(move || tick_once(&store, &journal_state))
                .await;
            match result {
                Ok(Ok(report)) if !(report.upserted == 0 && report.removed == 0) => {
                    tracing::info!(?report, "reconciled");
                }
                Ok(Ok(_)) => {}
                Ok(Err(e)) => tracing::warn!(error = %e, "reconcile tick failed"),
                Err(e) => tracing::warn!(error = %e, "reconcile tick panicked"),
            }
        }
    })
}

/// listener を開いて待ち受ける。
///
/// 有効な鍵が 1 件も無ければ bind の前に拒否する。framework の
/// `remote_server::serve` は同じチェックを内蔵しているが、こちらは `/mcp` を
/// 1 つの Router に merge するため `axum::serve` を自前で呼んでおり、その
/// チェックを経由しない —— ここで明示的に行う。認証なしで待ち受ける状態を
/// 作らない、という不変条件はどちらの経路でも同じにする。
pub async fn run(
    addr: std::net::SocketAddr,
    journal_dir: &Path,
    keys_path: &Path,
    state: Arc<ServerState>,
    journal_state: Arc<Mutex<JournalState>>,
    extra_allowed_hosts: &[String],
) -> anyhow::Result<()> {
    match state.keys() {
        Some(keys) if keys.has_usable_key() => {}
        // `state.keys() == None` can't actually happen here — `build_state`
        // always calls `.with_keys(...)`, so `state` never reaches `run`
        // without a `KeyStore` installed (possibly just an empty one).
        // Handled anyway, defensively, for symmetry with framework's own
        // `remote_server::serve`, whose check this mirrors — not because
        // there's a path that reaches it. Don't go looking for one.
        _ => {
            anyhow::bail!(
                "no usable API key configured in {}; run `sapphire-journal-server gen-key` \
                 first (an expired-only key file counts as none)",
                keys_path.display()
            );
        }
    }

    let cancel = CancellationToken::new();
    let mcp_hosts = allowed_hosts(addr, extra_allowed_hosts);
    let app = build_router(
        Arc::clone(&state),
        Arc::clone(&journal_state),
        journal_dir,
        cancel.clone(),
        &mcp_hosts,
    )?;

    // build_router がこの ws に対する WsStore を既に開いている
    // (`ServerState::workspace` はワークスペースごとにメモ化する) ので、ここで
    // 呼んでも新しいインスタンスは生まれない — tick が change log を分裂させる
    // ことはない。
    let ws = workspace_id(journal_dir)?;
    let store = state.workspace(&ws)?;
    let interval = match UserConfig::load() {
        // `sync_interval()` が `None` を返すのは `sync_interval_minutes = 0`
        // が明示的に設定されているとき (ファイル自体が無ければ `load()` は
        // 既定の 10 分入りの `UserConfig` を返すので、ここには来ない)。
        // `DEFAULT_TICK_INTERVAL` のドキュメント通り、このサーバではその 0 を
        // 尊重しない — ただし黙っては尊重しない、というだけ。
        Ok(cfg) if cfg.sync_interval().is_none() => {
            tracing::warn!(
                default_secs = DEFAULT_TICK_INTERVAL.as_secs(),
                "user config sets sync_interval_minutes = 0; on the client that disables \
                 MCP's own periodic re-index, but this server never starts that re-index \
                 at all — this tick is the only reconciliation path (the safety net for \
                 missed write notifications, hand edits, external tools), so it does not \
                 honor the 0 and runs at the default interval instead"
            );
            DEFAULT_TICK_INTERVAL
        }
        Ok(cfg) => cfg.sync_interval().unwrap_or(DEFAULT_TICK_INTERVAL),
        Err(_) => DEFAULT_TICK_INTERVAL,
    };
    let tick_handle = spawn_tick(store, Arc::clone(&journal_state), interval);

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind {addr}"))?;

    tracing::info!(
        %addr,
        journal = %journal_dir.display(),
        keys = %keys_path.display(),
        tick_interval_secs = interval.as_secs(),
        mcp_allowed_hosts = ?mcp_hosts,
        "sapphire-journal-server listening on /rpc and /mcp — private network only: \
         no TLS, no OAuth, tokens are stored in plaintext"
    );
    if extra_allowed_hosts.is_empty() && !addr.ip().is_loopback() {
        tracing::warn!(
            %addr,
            "listening beyond loopback with no --allowed-host: /rpc will answer any client, \
             but /mcp rejects every request whose Host header is not loopback or the bind \
             address itself (403 Forbidden). Pass --allowed-host for each name clients use."
        );
    }

    let result = axum::serve(listener, app)
        .with_graceful_shutdown(async move { cancel.cancelled().await })
        .await
        .context("server failed");

    tick_handle.abort();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowed_hosts_covers_the_bind_address_with_and_without_its_port() {
        let hosts = allowed_hosts("10.0.0.5:8080".parse().unwrap(), &[]);
        assert!(hosts.contains(&"10.0.0.5".to_owned()));
        assert!(hosts.contains(&"10.0.0.5:8080".to_owned()));
    }

    #[test]
    fn allowed_hosts_keeps_the_names_the_operator_named() {
        let hosts = allowed_hosts(
            "0.0.0.0:8080".parse().unwrap(),
            &["box.tailnet.ts.net".to_owned(), "nas.local:8080".to_owned()],
        );
        assert!(hosts.contains(&"box.tailnet.ts.net".to_owned()));
        assert!(hosts.contains(&"nas.local:8080".to_owned()));
    }

    #[test]
    fn allowed_hosts_drops_blank_entries() {
        // `--allowed-host ""` や `SAPPHIRE_JOURNAL_SERVER_ALLOWED_HOSTS=a,,b`
        // で空文字が混ざる。rmcp 側は空文字を無視するが、許可リストの中身は
        // 起動ログに出るので、ここで落としておく。
        let hosts = allowed_hosts(
            "127.0.0.1:8080".parse().unwrap(),
            &[String::new(), "  ".to_owned(), " keep.example ".to_owned()],
        );
        assert!(hosts.iter().all(|h| !h.trim().is_empty()));
        assert!(hosts.contains(&"keep.example".to_owned()), "{hosts:?}");
    }

    #[test]
    fn allowed_hosts_uses_the_bare_form_for_ipv6() {
        // rmcp は `Host: [::1]:8080` の角括弧を外して比較するので、許可リスト
        // 側も角括弧なしの形が要る。`SocketAddr::to_string` は角括弧つきの形
        // しか返さないため、`ip()` 側を別に入れておく必要がある。
        let hosts = allowed_hosts("[::1]:8080".parse().unwrap(), &[]);
        assert!(hosts.contains(&"::1".to_owned()), "{hosts:?}");
    }
}
