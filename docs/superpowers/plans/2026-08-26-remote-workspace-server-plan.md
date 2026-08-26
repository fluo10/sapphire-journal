# sapphire-journal リモートワークスペースサーバ — 実装計画

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `sapphire-journal-server` を新設し、framework の同期 API (`/rpc`) と journal 固有の MCP エンドポイント (`/mcp`) を 1 プロセスで提供する。人間が同期クライアント経由で、AI が MCP 経由で、同じ journal をエントリ単位で並行編集できるようにする。

**Architecture:** 1 つの axum アプリに framework の `router()` と journal-mcp の MCP サービスを載せ、両方を同じ鍵で `protect()` する。`ServerState::with_resolver` で journal ルートを origin に、journal 自身の retrieve ストアを注入した `WsStore` を解決する。MCP の書き込みは journal-mcp に足すオブザーバ経由で `record_local_write` に届き、取りこぼしは定期ティックの `reconcile` が回収する。

**Tech Stack:** Rust 2024 edition / axum 0.8 / rmcp 1.5 (streamable-http) / tokio 1 / clap 4 / sapphire-framework 0.1 (`remote-server` feature)

**Spec:** `docs/superpowers/specs/2026-08-25-remote-workspace-server-design.md`

## Global Constraints

- Rust 2024 edition。journal のワークスペースは `sapphire-journal-{cli,core,desktop,mcp}` に `sapphire-journal-server` を加える。
- framework 依存は**ファサード crate 経由**：`sapphire-framework = { version = "0.1", git = "…", branch = "main", default-features = false, features = ["remote-server"] }`。サブクレートを直接参照しない（`sapphire-timer-server` と同じ形）。
- トークンのプレフィクスは **`sjt_`**。鍵ファイルの形式・生成・検証はすべて framework の `KeyStore` が持つ。
- **`serve` はサブコマンド無し**（clap の `Option<Subcommand>`、`None` なら serve）。引数は `--journal-dir` / `--addr`（`SocketAddr` 1 本）/ `--keys`、環境変数は `SAPPHIRE_JOURNAL_SERVER_*`。
- **`ServerState::workspace()` で得た同じ `Arc<WsStore>` を MCP 側も使うこと。** 自前で `WsStore::with_config` を呼ぶと同じディレクトリに change log が 2 つできる。
- change log / track db / blob は `Journal::cache_dir()` 配下（`~/.cache/sapphire-journal/{uuid}/`）。journal ルートの外なので同期対象にならない。
- `WsStoreConfig.app_dir` は `Some(".sapphire-journal".to_owned())`。ワークスペースレベル設定は同期されるのが正しい。
- **プライベート網前提。** TLS も OAuth も範囲外。README と起動ログに明記する。
- テストは `cargo test --workspace`。**フォアグラウンドで実行し、数分かかっても待つこと**（redb/tantivy のコールドビルド）。

## 参照する framework の API（`main` にマージ済み・確認済み）

```rust
// sapphire_framework::remote_server::
pub struct WsStoreConfig {
    pub origin_dir: PathBuf,
    pub state_dir: PathBuf,
    pub retrieve: Option<Arc<dyn RetrieveStore + Send + Sync>>,
    pub app_dir: Option<String>,
}
impl ServerState {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self;
    pub fn with_keys(self, keys: Arc<KeyStore>) -> Self;
    pub fn with_resolver(self, f: impl Fn(&str) -> Result<WsStoreConfig> + Send + Sync + 'static) -> Self;
    pub fn workspace(&self, ws: &str) -> Result<Arc<WsStore>>;
}
pub fn router(state: Arc<ServerState>) -> Router;              // 認証適用済み
pub fn protect(state: Arc<ServerState>, router: Router) -> Router;
pub struct Authenticated { pub key_id: Uuid, pub label: Option<String> }
impl WsStore {
    pub fn record_local_write(&self, paths: &[String], updated_at: DateTime<Utc>) -> Result<Cursor>;
    pub fn reconcile(&self) -> Result<ReconcileReport>;
    pub fn snapshot(&self) -> Result<SnapshotResult>;
}
impl KeyStore {
    pub fn load(path: &Path) -> Result<Self>;
    pub fn generate(&mut self, prefix: &str, label: Option<String>, expires_at: Option<DateTime<Utc>>) -> Result<KeyEntry>;
    pub fn revoke(&mut self, selector: &str) -> Result<KeyEntry>;
    pub fn entries(&self) -> &[KeyEntry];
}
pub struct KeyEntry { pub token: String, pub id: Uuid, pub label: Option<String>,
                      pub created_at: DateTime<Utc>, pub expires_at: Option<DateTime<Utc>> }
```

## spec からの意図的な逸脱（実装可能性を確認した結果）

1. **MCP の書き込みラッパは journal-server ではなく journal-mcp に置く。** spec は「`ops` を呼んだ直後に `record_local_write` を呼ぶ薄いラッパを journal-server 側に置く」としているが、`ops::*` の呼び出しは journal-mcp のツールハンドラの内側（`server.rs` の 363 / 411 / 455 / 474 行）にあり、外から包む余地がない。journal-mcp に**書き込みオブザーバ**を足す（Task 3）。
2. **id の重複は push 時に拒否せず、事後の照合で解決する。** spec は「同じ id の別パスが live なら旧パスへの upsert を拒否して `conflicts` に載せる」としているが、push を処理するのは framework の `WsStore::push` で、journal が割り込む口がない。同期モデルがそもそも結果整合の LWW なので、定期ティックで重複を検出して新しい方を残す（Task 9）。framework に admission フックを足すより素直で、リポジトリを跨がない。

---

### Task 1: framework — retrieve ストアの共有と `UnknownWorkspace`

**別リポジトリ**（`sapphire-framework`）での作業。2 つとも journal 側から必要になるが
framework にしか置けない。

1. journal が自分の retrieve ストアを `WsStoreConfig` に注入できないと、同じファイル群に
   インデックスが 2 つできる。内部には既に `Arc` ハンドルがあるが private。
2. resolver が「知らないワークスペース」を拒否する手段がない。現状の `Error` は
   `Io / Redb / Retrieve / Blob / Track / Json / NotSyncable / KeyFile` だけで、どれも
   意味が合わない。拒否できないと、クライアントが適当な名前を送るたびに同じ origin に
   対する `WsStore` が別インスタンスとして生まれ、**change log が複数本になる**。

**Files:**
- Modify: `crates/sapphire-framework-retrieve/src/db.rs`
- Modify: `crates/sapphire-framework-remote-server/src/error.rs`
- Test: `crates/sapphire-framework-retrieve/src/db.rs`（`mod tests`）、`crates/sapphire-framework-remote-server/src/error.rs`（`mod tests`）

**Interfaces:**
- Consumes: 既存の private `RetrieveDb::store()`、既存の `Error::to_jsonrpc`
- Produces:
  - `pub fn RetrieveDb::shared(&self) -> Arc<dyn RetrieveStore + Send + Sync>`
  - `Error::UnknownWorkspace(String)`（JSON-RPC では `INVALID_PARAMS`）

- [ ] **Step 1: sapphire-framework に作業ブランチを作る**

```bash
cd ../sapphire-framework
git checkout main && git pull
git checkout -b feat/share-retrieve-store
```

- [ ] **Step 2: 失敗するテストを書く**

`crates/sapphire-framework-retrieve/src/db.rs` の `mod tests` に追記:

```rust
#[test]
fn shared_store_sees_documents_written_through_the_db() {
    let tmp = tempfile::tempdir().unwrap();
    let db = RetrieveDb::open(&tmp.path().join("retrieve.db")).unwrap();

    let shared = db.shared();
    db.upsert_document(&Document {
        id: 1,
        body: "hello".to_owned(),
        path: "a.md".to_owned(),
        chunks: None,
    })
    .unwrap();

    // 同じバックエンドを指しているので、共有ハンドル側からも見える。
    assert_eq!(shared.document_count().unwrap(), 1);
}
```

- [ ] **Step 3: テストが失敗することを確認する**

Run: `cargo test -p sapphire-framework-retrieve shared_store`
Expected: FAIL — `no method named shared`

- [ ] **Step 4: 最小実装を書く**

`db.rs` の `impl RetrieveDb` に追加（private な `store()` の隣）:

```rust
    /// バックエンドへの共有ハンドル。
    ///
    /// 同じプロセスの別コンポーネント（例: remote-server の `WsStore`）に、
    /// このデータベースと**同じ**インデックスを使わせるためのもの。別に開くと
    /// 同じファイル群に対してインデックスが二重にできる。
    pub fn shared(&self) -> Arc<dyn RetrieveStore + Send + Sync> {
        self.store()
    }
```

`store()` の戻り値型が `Arc<dyn RetrieveStore>` のままなら、`RetrieveStore` は
`Send + Sync` をスーパートレイトに持つのでそのまま返せる。型が合わないと言われた
場合は `store()` 側の戻り値型を `Arc<dyn RetrieveStore + Send + Sync>` に揃える。

- [ ] **Step 5: `UnknownWorkspace` を足す**

`crates/sapphire-framework-remote-server/src/error.rs` の `enum Error` に追加:

```rust
    /// 要求されたワークスペースをこのサーバは提供していない。
    ///
    /// resolver が名前で拒否するための種別。これが無いと、知らない名前でも
    /// 何らかの `WsStore` を返さざるを得ず、同じ origin に対して change log が
    /// 複数本できてしまう。
    #[error("unknown workspace: {0}")]
    UnknownWorkspace(String),
```

`to_jsonrpc` の分岐に加える（`NotSyncable` と同じく呼び出し側の誤りなので
`INVALID_PARAMS`）:

```rust
        let code = match self {
            Error::NotSyncable(_) | Error::UnknownWorkspace(_) => error_codes::INVALID_PARAMS,
            _ => error_codes::INTERNAL_ERROR,
        };
```

テスト:

```rust
#[test]
fn an_unknown_workspace_is_the_callers_mistake() {
    let err = Error::UnknownWorkspace("nope".to_owned());
    assert_eq!(err.to_jsonrpc().code, error_codes::INVALID_PARAMS);
}
```

- [ ] **Step 6: テストが通ることを確認する**

Run: `cargo test -p sapphire-framework-retrieve -p sapphire-framework-remote-server`
Expected: PASS

- [ ] **Step 7: コミットして PR を出す**

```bash
git add crates/sapphire-framework-retrieve/src/db.rs crates/sapphire-framework-remote-server/src/error.rs
git commit -m "feat: let an app share its retrieve store and reject unknown workspaces

An app that hands its retrieve store to remote-server's WsStore needs the
same backend on both sides, or the same files get indexed twice; the handle
already existed privately and this only names it.

A resolver also had no way to say a workspace name is not one it serves, so
it had to return some store for any name — and a second store over one origin
is a second change log."
git push -u origin feat/share-retrieve-store
gh pr create --base main --title "feat: share the retrieve store, reject unknown workspaces" --body "…"
```

**このタスクの PR がマージされるまで、Task 6 以降はビルドできない。** journal の依存は
framework の `main` を追うため。Task 2〜5 は framework に触れないので先行できる。

---

### Task 2: journal-mcp — MCP サービスを Router として取り出せるようにする

`serve_http` は Router を内部で組んで自分でリッスンしてしまうため、`/rpc` と同居させられない。

**Files:**
- Modify: `sapphire-journal-mcp/src/http.rs`
- Test: `sapphire-journal-mcp/tests/http_router.rs`（新規）

**Interfaces:**
- Consumes: 既存の `prepare_state`, `SapphireJournalServer::from_shared`
- Produces:
  - `pub fn mcp_router(state: Arc<Mutex<JournalState>>, cancel: CancellationToken, observer: Option<WriteObserver>) -> axum::Router`
  - `serve_http` はこれを使う薄いラッパになる（`observer` に `None` を渡す）

**`observer` 引数について**: Task 3 で足す書き込みオブザーバは、`mcp_router` が
ファクトリで `SapphireJournalServer` を作る以上、**ここから渡すしか経路が無い**。
Task 3 の実装より先に引数だけ用意しておく（この時点では常に `None`）。

- [ ] **Step 1: 失敗するテストを書く**

`sapphire-journal-mcp/tests/http_router.rs` を新規作成:

```rust
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
```

journal を作るライブラリ API は存在しない（`ensure_journal` は `pub(crate)`）。
`server.rs` の init 経路と同じ 3 手をテストヘルパとして書く:

```rust
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
```

`toml` を `[dev-dependencies]` に足すこと。

`sapphire-journal-mcp/Cargo.toml` の `[dev-dependencies]` に追記:

```toml
axum = { version = "0.8", default-features = false, features = ["http1", "tokio"] }
tower = { version = "0.5", features = ["util"] }
tempfile = "3"
tokio = { workspace = true, features = ["macros", "rt-multi-thread"] }
tokio-util = "0.7"
```

- [ ] **Step 2: テストが失敗することを確認する**

Run: `cargo test -p sapphire-journal-mcp --features http-server --test http_router`
Expected: FAIL — `cannot find function mcp_router`

- [ ] **Step 3: 最小実装を書く**

`sapphire-journal-mcp/src/http.rs` の `serve_http` を 2 つに割る:

```rust
/// MCP を `/mcp` に載せた [`axum::Router`] を返す。
///
/// 認証は**かけない**。呼び出し側が framework の `protect()` などで包むこと
/// （`sapphire-journal-server` がそうしている）。単体で外に晒すと、rmcp 既定の
/// ループバック制限しか守るものが無い。
pub fn mcp_router(
    shared_state: Arc<std::sync::Mutex<JournalState>>,
    cancel: CancellationToken,
    observer: Option<crate::server::WriteObserver>,
) -> axum::Router {
    let factory_state = Arc::clone(&shared_state);
    let factory = move || {
        let server = SapphireJournalServer::from_shared(Arc::clone(&factory_state));
        // セッションごとに新しいサーバが作られるので、オブザーバは毎回付け直す。
        Ok(match &observer {
            Some(o) => server.with_write_observer(Arc::clone(o)),
            None => server,
        })
    };

    let config = StreamableHttpServerConfig::default().with_cancellation_token(cancel);
    let http_service = StreamableHttpService::new(
        factory,
        Arc::new(LocalSessionManager::default()),
        config,
    );

    axum::Router::new().route_service("/mcp", http_service)
}
```

`serve_http` の本体を、状態を作って `mcp_router` を呼ぶだけに書き換える:

```rust
pub async fn serve_http(
    journal_dir: &Path,
    bind: &str,
    port: u16,
    cancel: CancellationToken,
) -> anyhow::Result<()> {
    let ip: IpAddr = bind
        .parse()
        .with_context(|| format!("invalid MCP HTTP bind address: {bind}"))?;
    let addr = SocketAddr::from((ip, port));

    let state = prepare_state(Some(journal_dir), false)?;
    let shared_state = Arc::new(std::sync::Mutex::new(state));
    let router = mcp_router(Arc::clone(&shared_state), cancel.clone(), None);

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind MCP HTTP server to {addr}"))?;
    tracing::info!("MCP HTTP server listening on http://{addr}/mcp");

    let sync_handle = spawn_periodic_reindex(shared_state);

    let serve_result = axum::serve(listener, router)
        .with_graceful_shutdown(async move { cancel.cancelled().await })
        .await;

    if let Some(handle) = sync_handle {
        handle.abort();
    }

    serve_result.context("MCP HTTP server failed")
}
```

`prepare_state` は `pub(crate)` のままでよい（`serve_http` から使う）。`mcp_router` は
状態を受け取る形なので、journal-server は自分で状態を作って渡す。

**Task 3 との順序**: `WriteObserver` 型と `with_write_observer` は Task 3 で入る。
このタスクを先にやる場合は `observer` 引数を一旦省き、Task 3 で足すこと — ただし
その場合 Task 3 のコミットが `mcp_router` のシグネチャも変えることになる。**先に
Task 3 をやってからこのタスクに来るほうが差分が素直。** 実行順は 3 → 2 を推奨する。

- [ ] **Step 4: テストが通ることを確認する**

Run: `cargo test -p sapphire-journal-mcp --features http-server`
Expected: PASS。desktop の既存利用（`serve_http`）が壊れていないことも
`cargo build --workspace` で確認する。

- [ ] **Step 5: コミット**

```bash
git add sapphire-journal-mcp/src/http.rs sapphire-journal-mcp/Cargo.toml sapphire-journal-mcp/tests/http_router.rs
git commit -m "feat(mcp): expose the MCP service as a Router

serve_http built its router privately and bound its own listener, so the
service could not share a port with anything else. The server crate needs to
mount it next to the framework's /rpc; serve_http now just wraps it."
```

---

### Task 3: journal-mcp — 書き込みオブザーバ

**Task 2 より先にやること。** `mcp_router` のシグネチャがこのタスクで入る型を使うため、
逆順だとシグネチャを二度変えることになる。

MCP のツールがファイルを書いたことを、外側（journal-server）が知る手段がない。

**Files:**
- Modify: `sapphire-journal-mcp/src/server.rs`
- Test: `sapphire-journal-mcp/src/server.rs`（`mod tests`）

**Interfaces:**
- Consumes: 既存の `SapphireJournalServer`
- Produces:
  - `pub type WriteObserver = Arc<dyn Fn(&[PathBuf]) + Send + Sync>`
  - `pub fn SapphireJournalServer::with_write_observer(self, observer: WriteObserver) -> Self`
  - 書き込み系ツール（create / update / fix / remove）が、影響したパスを 1 回の呼び出しで通知する

- [ ] **Step 1: 失敗するテストを書く**

`sapphire-journal-mcp/src/server.rs` の `mod tests` に追記（テストモジュールが無ければ作る）:

```rust
#[test]
fn a_rename_notifies_both_paths_in_one_call() {
    let observed: Arc<Mutex<Vec<Vec<PathBuf>>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&observed);

    let server = test_server().with_write_observer(Arc::new(move |paths: &[PathBuf]| {
        sink.lock().unwrap().push(paths.to_vec());
    }));

    let created = server.notify_write_for_test(&[PathBuf::from("2026/1_old.md")]);
    let _ = created;
    server.notify_write_for_test(&[
        PathBuf::from("2026/1_old.md"),
        PathBuf::from("2026/1_new.md"),
    ]);

    let calls = observed.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[1].len(), 2, "リネームは 1 回の通知に両方のパスを含める");
}

#[test]
fn no_observer_is_a_no_op() {
    let server = test_server();
    // オブザーバ未設定でも panic しないこと。
    server.notify_write_for_test(&[PathBuf::from("a.md")]);
}
```

`test_server()` は最小の `SapphireJournalServer` を作るヘルパ。`notify_write_for_test`
は通知メソッドの `#[cfg(test)]` な薄い別名にする（本体は private でよい）。

- [ ] **Step 2: テストが失敗することを確認する**

Run: `cargo test -p sapphire-journal-mcp with_write_observer`
Expected: FAIL — `no method named with_write_observer`

- [ ] **Step 3: 最小実装を書く**

`server.rs` に追加:

```rust
/// ファイルを書いたあとに呼ばれる通知。引数は影響したパス（ワークスペース相対
/// でも絶対でもよい — 変換は受け手の仕事）。
///
/// **1 回の呼び出しが 1 バッチ。** リネームは旧パスと新パスを同じ呼び出しに
/// 含める。分けて通知すると、同期側が一瞬エントリを失う。
pub type WriteObserver = Arc<dyn Fn(&[PathBuf]) + Send + Sync>;
```

`SapphireJournalServer` に `write_observer: Option<WriteObserver>` フィールドを足し、
`new` / `from_shared` では `None` で初期化する。ビルダを足す:

```rust
    /// 書き込み後の通知先を設定する。stdio 版では使わない。
    pub fn with_write_observer(mut self, observer: WriteObserver) -> Self {
        self.write_observer = Some(observer);
        self
    }

    /// 影響したパスを通知する。オブザーバ未設定なら何もしない。
    fn notify_write(&self, paths: &[PathBuf]) {
        if let Some(observer) = &self.write_observer {
            observer(paths);
        }
    }

    #[cfg(test)]
    pub(crate) fn notify_write_for_test(&self, paths: &[PathBuf]) {
        self.notify_write(paths);
    }
```

各ツールの書き込み直後に通知を挟む。**リネームは 1 回にまとめること**:

- `create_entry`（`server.rs:363` 付近）: `self.notify_write(&[dest.clone()]);`
- `update_entry`（`:411` 付近）: `ops::update_entry` は**リネームしたときだけ**
  `Some(new_path)` を返す。したがって

  ```rust
  let renamed = ops::update_entry(&path, &conn, fields)?;
  match &renamed {
      Some(new_path) => self.notify_write(&[path.clone(), new_path.clone()]),
      None => self.notify_write(&[path.clone()]),
  }
  let msg = if let Some(new_path) = renamed { /* 既存の文言 */ };
  ```
- `fix_entry`（`:455` 付近）: 同じ形（`Option<PathBuf>` を返す）。
- `remove_entry`（`:474` 付近）: `self.notify_write(&[path.clone()]);`

- [ ] **Step 4: テストが通ることを確認する**

Run: `cargo test -p sapphire-journal-mcp`
Expected: PASS

- [ ] **Step 5: コミット**

```bash
git add sapphire-journal-mcp/src/server.rs
git commit -m "feat(mcp): notify an observer after a tool writes

The server crate has to tell the change log which paths a tool touched, and
the ops calls are inside these handlers — there is nowhere outside to wrap.
A rename reports both paths in one call, because ops::update_entry only
returns Some(new_path) when it renamed."
```

---

### Task 4: `sapphire-journal-server` — crate の骨格と CLI

**Files:**
- Create: `sapphire-journal-server/Cargo.toml`
- Create: `sapphire-journal-server/src/lib.rs`
- Create: `sapphire-journal-server/src/main.rs`
- Create: `sapphire-journal-server/src/cli.rs`
- Modify: `Cargo.toml`（workspace members）
- Modify: `release-plz.toml`

**Interfaces:**
- Produces: `sapphire-journal-server` バイナリ（サブコマンド無しで serve）と、後続タスクが埋める lib

- [ ] **Step 1: crate を作り、ワークスペースに登録する**

`Cargo.toml`（ルート）の `members` に `"sapphire-journal-server"` を追加。

`sapphire-journal-server/Cargo.toml`:

```toml
[package]
name = "sapphire-journal-server"
edition.workspace = true
version = "0.1.0"
description = "Self-hosted remote workspace + MCP server for sapphire-journal"
license.workspace = true
repository.workspace = true
categories = ["command-line-utilities", "web-programming::http-server"]
keywords = ["markdown", "notes", "sync", "server", "mcp"]

[[bin]]
name = "sapphire-journal-server"
path = "src/main.rs"

[features]
default = ["redb-store", "fastembed-embed"]
redb-store      = ["sapphire-journal-core/redb-store",      "sapphire-journal-mcp/redb-store"]
lancedb-store   = ["sapphire-journal-core/lancedb-store",   "sapphire-journal-mcp/lancedb-store"]
fastembed-embed = ["sapphire-journal-core/fastembed-embed", "sapphire-journal-mcp/fastembed-embed"]

[dependencies]
sapphire-journal-core = { path = "../sapphire-journal-core", version = "0.12.0", default-features = false }
sapphire-journal-mcp  = { path = "../sapphire-journal-mcp",  version = "0.1.0",  default-features = false, features = ["http-server"] }
sapphire-framework = { version = "0.1", git = "https://github.com/fluo10/sapphire-framework", branch = "main", default-features = false, features = ["remote-server"] }
axum = { version = "0.8", default-features = false, features = ["http1", "tokio"] }
tokio = { workspace = true, features = ["rt-multi-thread", "macros", "signal", "time"] }
tokio-util = "0.7"
clap.workspace = true
anyhow.workspace = true
chrono.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true

[dev-dependencies]
tempfile = "3"
tower = { version = "0.5", features = ["util"] }
http-body-util = "0.1"
serde_json.workspace = true
```

`release-plz.toml` に追記（`desktop` のブロックにならい、`publish` の扱いはそれに合わせる）:

```toml
[[package]]
name = "sapphire-journal-server"
git_tag_name = "server-v{{ version }}"
git_release_name = "server-v{{ version }}"
```

- [ ] **Step 2: 失敗するテストを書く**

`sapphire-journal-server/src/cli.rs` を新規作成し、テストだけ先に書く:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser as _;

    #[test]
    fn no_subcommand_means_serve() {
        let cli = Cli::try_parse_from(["sapphire-journal-server"]).unwrap();
        assert!(cli.command.is_none(), "サブコマンド無しは serve");
    }

    #[test]
    fn addr_is_a_single_socket_addr() {
        let cli = Cli::try_parse_from(["sapphire-journal-server", "--addr", "0.0.0.0:9000"]).unwrap();
        assert_eq!(cli.addr.to_string(), "0.0.0.0:9000");
    }

    #[test]
    fn gen_key_takes_an_optional_label() {
        let cli = Cli::try_parse_from(["sapphire-journal-server", "gen-key"]).unwrap();
        assert!(matches!(cli.command, Some(Command::GenKey { label: None, .. })));

        let cli = Cli::try_parse_from(["sapphire-journal-server", "gen-key", "laptop"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::GenKey { label: Some(ref l), .. }) if l == "laptop"
        ));
    }

    #[test]
    fn revoke_key_requires_a_selector() {
        assert!(Cli::try_parse_from(["sapphire-journal-server", "revoke-key"]).is_err());
    }
}
```

- [ ] **Step 3: テストが失敗することを確認する**

Run: `cargo test -p sapphire-journal-server`
Expected: FAIL — `cannot find type Cli`

- [ ] **Step 4: 最小実装を書く**

`sapphire-journal-server/src/cli.rs` の先頭（`mod tests` の上）に:

```rust
//! コマンドライン引数。
//!
//! `serve` は既定動作なのでサブコマンドを持たない。鍵の管理だけがサブコマンド。

use std::net::SocketAddr;
use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "sapphire-journal-server",
    about = "Remote workspace + MCP server for sapphire-journal",
    version
)]
pub struct Cli {
    /// Journal root (the directory containing `.sapphire-journal/`).
    #[arg(long, env = "SAPPHIRE_JOURNAL_SERVER_DIR", value_name = "DIR")]
    pub journal_dir: PathBuf,

    /// Address to bind.
    #[arg(
        long,
        env = "SAPPHIRE_JOURNAL_SERVER_ADDR",
        default_value = "127.0.0.1:8080"
    )]
    pub addr: SocketAddr,

    /// Path to the API key file. Defaults to `<journal cache dir>/keys.toml`.
    #[arg(long, env = "SAPPHIRE_JOURNAL_SERVER_KEYS", value_name = "FILE")]
    pub keys: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Generate a new API key and print it.
    GenKey {
        /// A note for you — which host or person this key is for.
        label: Option<String>,
        /// Expire the key after this long, e.g. `90d`, `12h`.
        #[arg(long, value_name = "DURATION")]
        expires_in: Option<String>,
    },
    /// List the keys, with tokens masked.
    ListKeys,
    /// Remove a key by id or label.
    RevokeKey {
        /// The key's UUID, or its label when that is unambiguous.
        selector: String,
    },
}
```

`sapphire-journal-server/src/lib.rs`:

```rust
//! `sapphire-journal-server` — framework の同期 API と journal の MCP を
//! 1 プロセスで提供する。
//!
//! ## 前提
//!
//! **プライベート網（VPN / Tailscale / LAN）でのみ使うこと。** TLS も OAuth も
//! 持たない。認証は共有の bearer トークンだけで、鍵は平文で保存される。

pub mod cli;
```

`sapphire-journal-server/src/main.rs`:

```rust
use clap::Parser as _;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().init();
    let cli = sapphire_journal_server::cli::Cli::parse();
    // 実際のディスパッチは Task 5 / Task 6 で埋める。
    let _ = cli;
    Ok(())
}
```

- [ ] **Step 5: テストが通ることを確認する**

Run: `cargo test -p sapphire-journal-server`
Expected: PASS（4 件）

- [ ] **Step 6: コミット**

```bash
git add Cargo.toml release-plz.toml sapphire-journal-server
git commit -m "feat(server): scaffold sapphire-journal-server

A separate crate rather than a CLI subcommand: it deploys to a host rather
than a laptop, its feature set grows toward an embedder while the CLI's grows
leaner, and release-plz tags each crate on its own. serve is the default
action, so it takes no subcommand; only key management does."
```

---

### Task 5: 鍵管理サブコマンド

**Files:**
- Create: `sapphire-journal-server/src/keys.rs`
- Modify: `sapphire-journal-server/src/lib.rs`, `src/main.rs`
- Test: `sapphire-journal-server/src/keys.rs`（`mod tests`）

**Interfaces:**
- Consumes: framework の `KeyStore`, `KeyEntry`
- Produces:
  - `pub fn parse_duration(s: &str) -> anyhow::Result<chrono::Duration>`
  - `pub fn run(command: Command, keys_path: &Path) -> anyhow::Result<()>`
  - トークンのプレフィクスは `sjt_`

- [ ] **Step 1: 失敗するテストを書く**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_duration_accepts_days_hours_and_minutes() {
        assert_eq!(parse_duration("90d").unwrap(), chrono::Duration::days(90));
        assert_eq!(parse_duration("12h").unwrap(), chrono::Duration::hours(12));
        assert_eq!(parse_duration("30m").unwrap(), chrono::Duration::minutes(30));
    }

    #[test]
    fn parse_duration_rejects_junk() {
        assert!(parse_duration("").is_err());
        assert!(parse_duration("90").is_err(), "単位が要る");
        assert!(parse_duration("d90").is_err());
        assert!(parse_duration("-1d").is_err());
        assert!(parse_duration("90y").is_err());
    }

    #[test]
    fn gen_key_writes_a_prefixed_token_and_list_masks_it() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("keys.toml");

        run(Command::GenKey { label: Some("laptop".into()), expires_in: None }, &path).unwrap();

        let store = sapphire_framework::remote_server::KeyStore::load(&path).unwrap();
        assert_eq!(store.entries().len(), 1);
        assert!(store.entries()[0].token.starts_with("sjt_"));
        assert_eq!(store.entries()[0].label.as_deref(), Some("laptop"));
    }

    #[test]
    fn expires_in_becomes_an_absolute_time() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("keys.toml");

        run(Command::GenKey { label: None, expires_in: Some("90d".into()) }, &path).unwrap();

        let store = sapphire_framework::remote_server::KeyStore::load(&path).unwrap();
        let expires = store.entries()[0].expires_at.expect("期限が入っているはず");
        let expected = chrono::Utc::now() + chrono::Duration::days(90);
        assert!((expires - expected).num_seconds().abs() < 5);
    }

    #[test]
    fn revoke_key_removes_it() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("keys.toml");
        run(Command::GenKey { label: Some("gone".into()), expires_in: None }, &path).unwrap();

        run(Command::RevokeKey { selector: "gone".into() }, &path).unwrap();

        let store = sapphire_framework::remote_server::KeyStore::load(&path).unwrap();
        assert!(store.entries().is_empty());
    }
}
```

- [ ] **Step 2: テストが失敗することを確認する**

Run: `cargo test -p sapphire-journal-server keys`
Expected: FAIL — `cannot find function parse_duration`

- [ ] **Step 3: 最小実装を書く**

`sapphire-journal-server/src/keys.rs` の先頭に:

```rust
//! 鍵管理サブコマンド。
//!
//! 鍵ファイルの形式・生成・検証は framework の [`KeyStore`] が持つ。ここは
//! それを呼ぶ入口と、相対的な有効期間を絶対時刻へ直す変換だけ。

use std::path::Path;

use anyhow::{Context as _, bail};
use chrono::{Duration, Utc};
use sapphire_framework::remote_server::KeyStore;

use crate::cli::Command;

/// journal のトークンにつけるプレフィクス（sapphire-journal token）。
pub const TOKEN_PREFIX: &str = "sjt";

/// `90d` / `12h` / `30m` を [`Duration`] に直す。
///
/// 単位は必須。曖昧な `90` は拒否する — 秒なのか日なのか読めない値を鍵の
/// 有効期限に使わせない。
pub fn parse_duration(s: &str) -> anyhow::Result<Duration> {
    let (value, unit) = s.split_at(
        s.find(|c: char| !c.is_ascii_digit())
            .with_context(|| format!("duration needs a unit (d/h/m): {s:?}"))?,
    );
    if value.is_empty() {
        bail!("duration needs a number: {s:?}");
    }
    let n: i64 = value.parse().with_context(|| format!("bad duration: {s:?}"))?;
    match unit {
        "d" => Ok(Duration::days(n)),
        "h" => Ok(Duration::hours(n)),
        "m" => Ok(Duration::minutes(n)),
        other => bail!("unknown duration unit {other:?} in {s:?} (use d, h or m)"),
    }
}

/// 鍵サブコマンドを実行する。
pub fn run(command: Command, keys_path: &Path) -> anyhow::Result<()> {
    let mut store = KeyStore::load(keys_path)
        .with_context(|| format!("loading API keys from {}", keys_path.display()))?;

    match command {
        Command::GenKey { label, expires_in } => {
            let expires_at = expires_in
                .as_deref()
                .map(parse_duration)
                .transpose()?
                // 相対指定は生成時に絶対時刻へ直して保存する。ファイルには絶対
                // 時刻だけを持たせるほうが、後から読んだときに曖昧さがない。
                .map(|d| Utc::now() + d);
            let entry = store.generate(TOKEN_PREFIX, label, expires_at)?;
            println!("{}", entry.token);
            eprintln!(
                "id {}  created {}{}",
                entry.id,
                entry.created_at.to_rfc3339(),
                entry
                    .expires_at
                    .map(|e| format!("  expires {}", e.to_rfc3339()))
                    .unwrap_or_default()
            );
        }
        Command::ListKeys => {
            let now = Utc::now();
            for e in store.entries() {
                let masked = format!("{}…", &e.token[..e.token.len().min(12)]);
                let state = if e.is_expired(now) { " (expired)" } else { "" };
                println!(
                    "{}  {}  {}  {}{}",
                    e.id,
                    masked,
                    e.created_at.to_rfc3339(),
                    e.label.as_deref().unwrap_or("-"),
                    state
                );
            }
        }
        Command::RevokeKey { selector } => {
            let removed = store.revoke(&selector)?;
            eprintln!("revoked {} ({})", removed.id, removed.label.as_deref().unwrap_or("-"));
        }
    }
    Ok(())
}
```

`lib.rs` に `pub mod keys;` を追加し、`main.rs` でディスパッチする:

```rust
fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().init();
    let cli = Cli::parse();
    let keys_path = resolve_keys_path(&cli)?;   // Task 6 で journal の cache dir 由来にする
    match cli.command {
        Some(command) => sapphire_journal_server::keys::run(command, &keys_path),
        None => todo!("serve — Task 6"),
    }
}
```

- [ ] **Step 4: テストが通ることを確認する**

Run: `cargo test -p sapphire-journal-server`
Expected: PASS

- [ ] **Step 5: コミット**

```bash
git add sapphire-journal-server
git commit -m "feat(server): key management subcommands

Tokens carry an sjt_ prefix, following the syt_/ghp_/sk_live_ convention of
an abbreviation plus a use letter. A relative --expires-in is converted to an
absolute time at generation, so the file only ever holds absolute times; a
bare number without a unit is rejected rather than guessed at."
```

---

### Task 6: サーバ本体 — resolver、Router 合成、起動

**Task 1 の framework PR がマージされている必要がある。**

**Files:**
- Create: `sapphire-journal-server/src/serve.rs`
- Modify: `sapphire-journal-server/src/lib.rs`, `src/main.rs`
- Test: `sapphire-journal-server/tests/routes.rs`（新規）

**Interfaces:**
- Consumes: Task 1 の `RetrieveDb::shared`、Task 2 の `mcp_router`、framework の `ServerState` / `protect`
- Produces:
  - `pub fn build_state(journal_dir: &Path, keys_path: &Path) -> anyhow::Result<Arc<ServerState>>`
  - `pub fn build_router(state: Arc<ServerState>, journal_state: Arc<Mutex<JournalState>>, cancel: CancellationToken) -> anyhow::Result<Router>`（Task 7 でオブザーバを繋ぐとき `state.workspace()` が `Result` を返すので、最初から `Result` にしておく）
  - `pub async fn serve(...) -> anyhow::Result<()>`
  - ワークスペース名は `Journal::journal_id()` の UUID 文字列

- [ ] **Step 1: 失敗するテストを書く**

`sapphire-journal-server/tests/routes.rs`:

```rust
//! `/rpc` と `/mcp` が同じ Router に載り、同じ鍵で守られていることを確かめる。

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use tower::ServiceExt as _;

mod harness;   // 下の Step 3 で書く共通ヘルパ

#[tokio::test]
async fn both_routes_reject_a_request_without_a_token() {
    let h = harness::spawn().await;

    for uri in ["/rpc", "/mcp"] {
        let response = h
            .router()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "{uri} が無認証で通っている"
        );
    }
}

#[tokio::test]
async fn rpc_answers_a_snapshot_with_the_key() {
    let h = harness::spawn().await;

    let response = h.rpc("workspace.snapshot", serde_json::json!({"ws": h.ws()})).await;

    assert!(response.get("error").is_none(), "{response:?}");
    let result = &response["result"];
    assert!(result.get("generation").is_some(), "世代 ID が返っていない");
}
```

- [ ] **Step 2: テストが失敗することを確認する**

Run: `cargo test -p sapphire-journal-server --test routes`
Expected: FAIL — `harness` も `build_router` も存在しない

- [ ] **Step 3: テストハーネスを書く**

`sapphire-journal-server/tests/harness/mod.rs`:

```rust
//! テスト用に journal を 1 つ作り、鍵を 1 本発行してサーバを組み立てる。

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Body;
use axum::http::{Request, header};
use http_body_util::BodyExt as _;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt as _;

pub struct Harness {
    _tmp: tempfile::TempDir,
    pub journal_dir: PathBuf,
    pub token: String,
    pub ws: String,
    router: Router,
}

pub async fn spawn() -> Harness {
    let tmp = tempfile::tempdir().unwrap();
    let journal_dir = tmp.path().join("journal");
    sapphire_journal_core::init_app_context();
    init_journal(&journal_dir);

    let keys_path = tmp.path().join("keys.toml");
    let mut store = sapphire_framework::remote_server::KeyStore::load(&keys_path).unwrap();
    let token = store
        .generate(sapphire_journal_server::keys::TOKEN_PREFIX, Some("test".into()), None)
        .unwrap()
        .token;

    let journal_state = sapphire_journal_server::serve::open_journal_state(&journal_dir).unwrap();
    let state = sapphire_journal_server::serve::build_state(
        &journal_dir,
        &keys_path,
        Arc::clone(&journal_state),
    )
    .unwrap();
    let ws = sapphire_journal_server::serve::workspace_id(&journal_dir).unwrap();
    let router = sapphire_journal_server::serve::build_router(
        Arc::clone(&state),
        journal_state,
        CancellationToken::new(),
    )
    .unwrap();

    Harness { _tmp: tmp, journal_dir, token, ws, router }
}

/// `.sapphire-journal/` を掘って journal にする。`ensure_journal` の init 分岐と同じ。
fn init_journal(root: &std::path::Path) {
    use sapphire_journal_core::journal::JournalConfig;
    let journal_dir = root.join(".sapphire-journal");
    std::fs::create_dir_all(&journal_dir).unwrap();
    std::fs::write(
        journal_dir.join("config.toml"),
        toml::to_string_pretty(&JournalConfig::default()).unwrap(),
    )
    .unwrap();
    std::fs::write(journal_dir.join(".gitignore"), "cache/\n").unwrap();
}

impl Harness {
    pub fn router(&self) -> Router {
        self.router.clone()
    }
    pub fn ws(&self) -> &str {
        &self.ws
    }

    /// 鍵つきで JSON-RPC を 1 回叩く。
    pub async fn rpc(&self, method: &str, params: serde_json::Value) -> serde_json::Value {
        let body = serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": method, "params": params
        });
        let response = self
            .router()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/rpc")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, format!("Bearer {}", self.token))
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }
}
```

- [ ] **Step 4: 最小実装を書く**

`sapphire-journal-server/src/serve.rs`:

```rust
//! サーバの組み立てと起動。

use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::Context as _;
use axum::Router;
use sapphire_framework::remote_server::{KeyStore, ServerState, WsStoreConfig, protect, router};
use sapphire_journal_core::journal::Journal;
use sapphire_journal_core::journal_state::JournalState;
use tokio_util::sync::CancellationToken;

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

/// `/rpc` と `/mcp` を 1 つの Router に束ね、両方に同じ鍵をかける。
pub fn build_router(
    state: Arc<ServerState>,
    journal_state: Arc<Mutex<JournalState>>,
    cancel: CancellationToken,
) -> anyhow::Result<Router> {
    // オブザーバは Task 7 で繋ぐ。ここでは None。
    let mcp = sapphire_journal_mcp::http::mcp_router(journal_state, cancel, None);
    // `router()` は認証適用済み。`/mcp` は自分で protect に通す — 片方だけ
    // 守られている状態を作らない。
    Ok(router(Arc::clone(&state)).merge(protect(state, mcp)))
}

/// listener を開いて待ち受ける。
pub async fn run(
    addr: std::net::SocketAddr,
    journal_dir: &Path,
    keys_path: &Path,
    state: Arc<ServerState>,
    journal_state: Arc<Mutex<JournalState>>,
) -> anyhow::Result<()> {
    let cancel = CancellationToken::new();
    let app = build_router(Arc::clone(&state), Arc::clone(&journal_state), cancel.clone())?;

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind {addr}"))?;

    tracing::info!(
        %addr,
        journal = %journal_dir.display(),
        keys = %keys_path.display(),
        "sapphire-journal-server listening on /rpc and /mcp — private network only: \
         no TLS, no OAuth, tokens are stored in plaintext"
    );

    // 定期ティックは Task 8 でここに足す。
    axum::serve(listener, app)
        .with_graceful_shutdown(async move { cancel.cancelled().await })
        .await
        .context("server failed")
}
```

**`journal_id()` は初回呼び出しで `config.toml` を書き換える**（UUID が未設定なら
採番して保存する）。`build_state` は resolver を組む前にこれを呼ぶので、書き込みは
起動時に 1 回だけ起き、`workspaces` の Mutex の下では起きない。この順序を崩さないこと。

また `config.toml` への書き込みは origin の中で起きるが、`app_dir` が
`.sapphire-journal` を許可しているので同期に載る。ワークスペースレベル設定は端末間で
共有したいので、これは意図した挙動。

`main.rs` の `None => todo!()` を埋める:

```rust
        None => {
            let journal_state = serve::open_journal_state(&cli.journal_dir)?;
            let state = serve::build_state(&cli.journal_dir, &keys_path, Arc::clone(&journal_state))?;
            serve::run(cli.addr, &cli.journal_dir, &keys_path, state, journal_state).await
        }
```

`main` は `#[tokio::main]` にする必要がある（Task 4 では同期の `fn main` にしてある）。
鍵サブコマンドは同期のままでよいので、`serve` の分岐だけランタイム上で走らせる形にする。

既定の鍵ファイルパスは `serve::default_keys_path(&cli.journal_dir)?`。`--keys` が
与えられていればそちらを使う。

- [ ] **Step 5: テストが通ることを確認する**

Run: `cargo test -p sapphire-journal-server`
Expected: PASS

- [ ] **Step 6: コミット**

```bash
git add sapphire-journal-server
git commit -m "feat(server): serve /rpc and /mcp from one router

Both mount over the same journal directory and are protected by the same
keys, so a deployment cannot end up with sync guarded and MCP open. The
resolver hands the framework the journal root as origin and the journal's own
retrieve store, so one set of files gets one index."
```

---

### Task 7: MCP の書き込みを change log に載せる

**Files:**
- Modify: `sapphire-journal-server/src/serve.rs`
- Test: `sapphire-journal-server/tests/write_path.rs`（新規）

**Interfaces:**
- Consumes: Task 3 の `with_write_observer`、framework の `WsStore::record_local_write`
- Produces: MCP 経由で作られたエントリが `changes.pull` に出る

- [ ] **Step 1: 失敗するテストを書く**

`sapphire-journal-server/tests/write_path.rs`:

```rust
mod harness;

/// この計画の目的そのもの: AI が MCP で書いたものが、人間側の同期に出る。
#[tokio::test]
async fn an_entry_written_through_mcp_appears_in_changes_pull() {
    let h = harness::spawn().await;

    // MCP のツールを直接叩くのは重いので、オブザーバが繋がっている経路を
    // サーバ内部の同じ入口で再現する。
    h.write_entry_through_mcp("first note").await;

    let pulled = h
        .rpc(
            "changes.pull",
            serde_json::json!({"ws": h.ws(), "since": 0, "limit": 10}),
        )
        .await;

    let changes = pulled["result"]["changes"].as_array().unwrap();
    assert!(
        changes.iter().any(|c| c["path"].as_str().unwrap().ends_with(".md")),
        "MCP で書いたエントリが pull に出ていない: {changes:?}"
    );
}

#[tokio::test]
async fn a_title_change_reports_both_paths_and_leaves_one_live_entry() {
    let h = harness::spawn().await;
    let path = h.write_entry_through_mcp("before").await;

    h.retitle_through_mcp(&path, "after").await;

    let snapshot = h
        .rpc("workspace.snapshot", serde_json::json!({"ws": h.ws()}))
        .await;
    let docs = snapshot["result"]["docs"].as_array().unwrap();
    let md: Vec<_> = docs
        .iter()
        .filter(|d| d["path"].as_str().unwrap().ends_with(".md"))
        .collect();
    assert_eq!(md.len(), 1, "リネーム後にエントリが 2 つ見えている: {md:?}");
}
```

`write_entry_through_mcp` / `retitle_through_mcp` はハーネスに足す。MCP の
ツールを通すのが重すぎる場合は、`SapphireJournalServer` のツールメソッドを
直接呼ぶ形でよい — **オブザーバを通る経路であることが要件**で、HTTP を通る
ことは要件ではない。

- [ ] **Step 2: テストが失敗することを確認する**

Run: `cargo test -p sapphire-journal-server --test write_path`
Expected: FAIL — pull に何も出ない（オブザーバが未接続）

- [ ] **Step 3: 最小実装を書く**

オブザーバは Task 2 で用意した `mcp_router` の第 3 引数から渡す。`mcp_router` が
セッションごとにファクトリで `SapphireJournalServer` を作る以上、外から後付けする
経路は無い。`build_router` を次のように変える:

```rust
pub fn build_router(
    state: Arc<ServerState>,
    journal_state: Arc<Mutex<JournalState>>,
    cancel: CancellationToken,
) -> anyhow::Result<Router> {
    let ws = workspace_id(journal_dir)?;
    // /rpc と同じインスタンス。ここで with_config を自分で呼ぶと change log が 2 本になる。
    let store = state.workspace(&ws)?;
    let origin = Journal::from_root(journal_dir.to_path_buf())?.root;
    let observer = write_observer(store, origin);

    let mcp = sapphire_journal_mcp::http::mcp_router(journal_state, cancel, Some(observer));
    Ok(router(Arc::clone(&state)).merge(protect(state, mcp)))
}
```

`build_router` は `journal_dir: &Path` を受け取る形に変える（Task 6 の呼び出し側と
ハーネスも合わせること）。
```

オブザーバの中身:

```rust
/// MCP が書いたパスを change log に載せる。
///
/// パスは絶対で来るので origin 相対の POSIX に直す。`record_local_write` は
/// 1 回の呼び出しが 1 バッチなので、リネームの旧・新はここで 1 回にまとまる
/// （通知側が 1 回で渡してくる）。
fn write_observer(store: Arc<WsStore>, origin: PathBuf) -> sapphire_journal_mcp::WriteObserver {
    Arc::new(move |paths: &[PathBuf]| {
        let rel: Vec<String> = paths
            .iter()
            .filter_map(|p| p.strip_prefix(&origin).ok())
            .map(|p| {
                p.components()
                    .map(|c| c.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/")
            })
            .collect();
        if rel.is_empty() {
            return;
        }
        if let Err(e) = store.record_local_write(&rel, chrono::Utc::now()) {
            // 落とさない。取りこぼしは Task 8 の定期 reconcile が回収する。
            tracing::warn!(error = %e, paths = ?rel, "failed to record an MCP write");
        }
    })
}
```

`store` は `state.workspace(&ws)?` で得た `Arc<WsStore>`。**`WsStore::with_config`
を自分で呼ばないこと** — `/rpc` と同じインスタンスでなければ change log が 2 本になる。

- [ ] **Step 4: テストが通ることを確認する**

Run: `cargo test -p sapphire-journal-server`
Expected: PASS

- [ ] **Step 5: コミット**

```bash
git add sapphire-journal-server
git commit -m "feat(server): publish MCP writes to the change log

Without this an AI's edits are invisible to the syncing client, and the next
push overwrites them. The observer hands record_local_write the same Arc the
/rpc path uses, so one change log covers both writers. A failure is logged
rather than propagated — the reconcile tick is the net."
```

---

### Task 8: 走査を 1 つのティックに統合する

サーバ上では放っておくと走査が 3 つ並ぶ（MCP の定期再インデックス、journal の
キャッシュ同期、framework の `reconcile`）。

**Files:**
- Modify: `sapphire-journal-server/src/serve.rs`
- Test: `sapphire-journal-server/tests/tick.rs`（新規）

**Interfaces:**
- Produces: `pub fn spawn_tick(store: Arc<WsStore>, journal_state: Arc<Mutex<JournalState>>, interval: Duration) -> JoinHandle<()>`

- [ ] **Step 1: 失敗するテストを書く**

```rust
mod harness;

#[tokio::test]
async fn the_tick_picks_up_a_file_written_behind_the_server() {
    let h = harness::spawn().await;

    // 誰も通知してくれない書き込み（外部ツール、手作業）。
    let path = h.journal_dir.join("2026").join("handwritten.md");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "---\nid: 01J000000000000000000000\n---\n\nby hand\n").unwrap();

    h.tick_once().await;

    let snapshot = h.rpc("workspace.snapshot", serde_json::json!({"ws": h.ws()})).await;
    let docs = snapshot["result"]["docs"].as_array().unwrap();
    assert!(
        docs.iter().any(|d| d["path"].as_str().unwrap().ends_with("handwritten.md")),
        "手書きのファイルが回収されていない"
    );
}

#[tokio::test]
async fn a_second_tick_is_quiet() {
    let h = harness::spawn().await;
    h.write_entry_through_mcp("note").await;

    h.tick_once().await;
    let report = h.tick_once().await;

    assert_eq!(report.upserted, 0, "変化していないのに再検出している");
    assert_eq!(report.removed, 0);
}
```

`tick_once` はハーネスに足す（1 回分の処理を同期的に走らせる）。

- [ ] **Step 2: テストが失敗することを確認する**

Run: `cargo test -p sapphire-journal-server --test tick`
Expected: FAIL — `tick_once` も `spawn_tick` も存在しない

- [ ] **Step 3: 最小実装を書く**

```rust
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
    journal_state.lock().unwrap().sync()?;
    let report = store.reconcile()?;
    Ok(report)
}

/// [`tick_once`] を `interval` ごとに回す。起動直後に 1 回走らせてから待つ。
pub fn spawn_tick(
    store: Arc<WsStore>,
    journal_state: Arc<Mutex<JournalState>>,
    interval: std::time::Duration,
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
                Ok(Ok(report)) if !report.is_empty() => {
                    tracing::info!(?report, "reconciled");
                }
                Ok(Ok(_)) => {}
                Ok(Err(e)) => tracing::warn!(error = %e, "reconcile tick failed"),
                Err(e) => tracing::warn!(error = %e, "reconcile tick panicked"),
            }
        }
    })
}
```

`ReconcileReport` に `is_empty()` が無ければ `report.upserted == 0 && report.removed == 0`
で書くこと。間隔は `UserConfig::sync_interval()` があればそれを使い、無ければ
5 分を既定にする。

`serve::run` から `spawn_tick` を呼び、`spawn_periodic_reindex` は**呼ばない**。

- [ ] **Step 4: テストが通ることを確認する**

Run: `cargo test -p sapphire-journal-server`
Expected: PASS

- [ ] **Step 5: コミット**

```bash
git add sapphire-journal-server
git commit -m "feat(server): one reconciliation tick, not three

The MCP transport, the journal cache and the framework's reconcile each
wanted their own periodic walk over the same files, and only one of them
updated the change log. The server runs a single tick and does not start the
MCP re-index."
```

---

### Task 9: 同じ id を持つ重複エントリの解消

タイトル変更でパスが変わったあと、古いカーソルのクライアントが**旧パスへの編集**を
push すると、パス単位 LWW では旧パスが復活して同じ id のファイルが 2 つになる。

spec は push 時の拒否を想定していたが、push を処理するのは framework の
`WsStore::push` で journal が割り込む口がない。同期モデルが結果整合の LWW なので、
**ティックで検出して新しい方を残す**。

**Files:**
- Modify: `sapphire-journal-server/src/serve.rs`
- Create: `sapphire-journal-server/src/dedupe.rs`
- Test: `sapphire-journal-server/src/dedupe.rs`（`mod tests`）+ `tests/tick.rs`

**Interfaces:**
- Produces:
  - `pub fn find_duplicate_ids(paths: &[String]) -> Vec<Vec<String>>`
  - `pub fn resolve_duplicates(store: &WsStore, origin: &Path) -> anyhow::Result<usize>`

- [ ] **Step 1: 失敗するテストを書く**

`sapphire-journal-server/src/dedupe.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_paths_that_share_an_id() {
        let paths = vec![
            "2026/01J1_old-title.md".to_owned(),
            "2026/01J1_new-title.md".to_owned(),
            "2026/01J2_other.md".to_owned(),
        ];

        let groups = find_duplicate_ids(&paths);

        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].len(), 2);
        assert!(groups[0].iter().all(|p| p.contains("01J1")));
    }

    #[test]
    fn a_bare_id_filename_still_groups() {
        // スラグが空のときファイル名は `{id}.md` になる。
        let paths = vec!["2026/01J1.md".to_owned(), "2026/01J1_titled.md".to_owned()];
        assert_eq!(find_duplicate_ids(&paths).len(), 1);
    }

    #[test]
    fn unique_ids_produce_no_groups() {
        let paths = vec!["2026/01J1_a.md".to_owned(), "2026/01J2_b.md".to_owned()];
        assert!(find_duplicate_ids(&paths).is_empty());
    }

    #[test]
    fn non_entry_files_are_ignored() {
        let paths = vec![
            ".sapphire-journal/config.toml".to_owned(),
            "README.md".to_owned(),
        ];
        assert!(find_duplicate_ids(&paths).is_empty());
    }
}
```

`tests/tick.rs` に統合テストを追記:

```rust
#[tokio::test]
async fn a_stale_push_to_the_old_path_is_resolved_to_one_entry() {
    let h = harness::spawn().await;
    let path = h.write_entry_through_mcp("before").await;
    h.tick_once().await;
    let old_rel = h.relative(&path);

    // タイトル変更でリネーム。
    h.retitle_through_mcp(&path, "after").await;

    // 古いカーソルのクライアントが旧パスへ push してくる。
    h.rpc(
        "changes.push",
        serde_json::json!({
            "ws": h.ws(), "base_cursor": 0,
            "changes": [{
                "path": old_rel, "kind": "upsert",
                "body": "stale edit", "updated_at": chrono::Utc::now().to_rfc3339()
            }]
        }),
    )
    .await;

    h.tick_once().await;

    let snapshot = h.rpc("workspace.snapshot", serde_json::json!({"ws": h.ws()})).await;
    let md: Vec<_> = snapshot["result"]["docs"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["path"].as_str().unwrap().ends_with(".md"))
        .collect();
    assert_eq!(md.len(), 1, "同じ id のエントリが 2 つ残っている: {md:?}");
}
```

- [ ] **Step 2: テストが失敗することを確認する**

Run: `cargo test -p sapphire-journal-server dedupe`
Expected: FAIL — `cannot find function find_duplicate_ids`

- [ ] **Step 3: 最小実装を書く**

`sapphire-journal-server/src/dedupe.rs`:

```rust
//! 同じ `GrainId` を持つエントリが 2 つ live になった状態の解消。
//!
//! エントリのファイル名は `{id}_{slug}.md`（スラグが空なら `{id}.md`）なので、
//! タイトルを変えるとパスが変わる。リネーム前のパスへ古いカーソルのクライアント
//! が push すると、パス単位 LWW では旧パスが復活し、同じ id のファイルが 2 つに
//! なる。
//!
//! push の時点で拒否する口が framework 側に無いため、ティックで検出して新しい
//! ほうを残す。同期モデルがそもそも結果整合の LWW なので、事後の収束で筋が通る。

use std::collections::HashMap;
use std::path::Path;

use sapphire_framework::remote_server::WsStore;

/// ワークスペース相対パスから `{id}` を取り出す。エントリらしくなければ `None`。
fn entry_id(path: &str) -> Option<&str> {
    let file = path.rsplit('/').next()?;
    let stem = file.strip_suffix(".md")?;
    let id = stem.split('_').next()?;
    // GrainId は英数字。空や明らかに違うものは弾く。
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    Some(id)
}

/// 同じ id を共有するパスのグループ（2 件以上のものだけ）。
pub fn find_duplicate_ids(paths: &[String]) -> Vec<Vec<String>> {
    let mut by_id: HashMap<&str, Vec<String>> = HashMap::new();
    for p in paths {
        if let Some(id) = entry_id(p) {
            by_id.entry(id).or_default().push(p.clone());
        }
    }
    by_id.into_values().filter(|g| g.len() > 1).collect()
}

/// live なドキュメントを走査し、同じ id の重複を解消する。残すのは
/// `updated_at` が新しいほう。戻り値は削除した件数。
pub fn resolve_duplicates(store: &WsStore, origin: &Path) -> anyhow::Result<usize> {
    let snapshot = store.snapshot()?;
    let paths: Vec<String> = snapshot.docs.iter().map(|c| c.path.clone()).collect();
    let mut removed = 0usize;

    for group in find_duplicate_ids(&paths) {
        // 各パスの updated_at を snapshot から引く。
        let mut with_time: Vec<(&String, chrono::DateTime<chrono::Utc>)> = group
            .iter()
            .filter_map(|p| {
                snapshot
                    .docs
                    .iter()
                    .find(|c| &c.path == p)
                    .map(|c| (p, c.updated_at))
            })
            .collect();
        with_time.sort_by_key(|(_, t)| *t);

        // 最後の 1 つ（最も新しい）以外を消す。
        let doomed: Vec<String> = with_time
            .iter()
            .rev()
            .skip(1)
            .map(|(p, _)| (*p).clone())
            .collect();

        for path in &doomed {
            let abs = origin.join(path.replace('/', std::path::MAIN_SEPARATOR_STR));
            match std::fs::remove_file(&abs) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
            tracing::info!(path, "removed a duplicate of an entry id");
            removed += 1;
        }
        if !doomed.is_empty() {
            store.record_local_write(&doomed, chrono::Utc::now())?;
        }
    }
    Ok(removed)
}
```

`tick_once` の `reconcile` のあとに `resolve_duplicates` を呼ぶ。

- [ ] **Step 4: テストが通ることを確認する**

Run: `cargo test -p sapphire-journal-server`
Expected: PASS

- [ ] **Step 5: コミット**

```bash
git add sapphire-journal-server
git commit -m "feat(server): collapse two entries that share an id

A title change renames the file, so a client pushing from a stale cursor
resurrects the old path and the same id ends up in two files. The framework
has no admission hook to reject that at push time, and the sync model is
eventually-consistent anyway, so the tick keeps the newer one."
```

---

### Task 10: 仕上げ — CRLF、README、起動ログ

**Files:**
- Create: `sapphire-journal-server/README.md`
- Modify: `sapphire-journal-server/src/serve.rs`
- Test: `sapphire-journal-server/tests/crlf.rs`（新規）

- [ ] **Step 1: 失敗するテストを書く**

```rust
mod harness;

/// この環境は core.autocrlf=true で `.gitattributes` が無いため、チェックアウト
/// した `.md` は CRLF になる。サーバ経由で読むときも壊れないこと。
#[tokio::test]
async fn a_crlf_entry_round_trips_through_the_server() {
    let h = harness::spawn().await;

    let path = h.journal_dir.join("2026").join("01J9_crlf.md");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        "---\r\nid: 01J9\r\ntitle: crlf\r\n---\r\n\r\nbody\r\n",
    )
    .unwrap();

    h.tick_once().await;

    let snapshot = h.rpc("workspace.snapshot", serde_json::json!({"ws": h.ws()})).await;
    let doc = snapshot["result"]["docs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["path"].as_str().unwrap().ends_with("01J9_crlf.md"))
        .expect("CRLF のエントリが同期に載っていない");
    assert!(doc["body"].as_str().unwrap().contains("body"));
}
```

- [ ] **Step 2: テストを実行する**

Run: `cargo test -p sapphire-journal-server --test crlf`
Expected: 通るはず（`split_frontmatter` は CRLF 対応済み）。**落ちた場合は
実装を直すのではなく、まず落ちた理由を報告すること** — パーサ側の退行かもしれない。

- [ ] **Step 3: 起動ログと README を書く**

`serve::run` の起動ログに、待ち受けアドレス・journal ルート・鍵ファイルのパスと、
**プライベート網前提である旨**を出す。

`sapphire-journal-server/README.md` には最低限、次を書く:

- これが何か（同期 API と MCP を 1 プロセスで出す自ホスト用サーバ）
- **プライベート網（VPN / Tailscale / LAN）でのみ使うこと。TLS も OAuth も無い**
- `gen-key` で鍵を発行し、クライアントに `Authorization: Bearer <token>` を設定させる手順
- 鍵ファイルは journal ルートの外（cache dir）にあり、平文であること
- 鍵が 1 本も無いと起動しないこと

- [ ] **Step 4: 全体を確認する**

Run: `cargo test --workspace`
Expected: PASS

Run: `cargo build --workspace`
Expected: 警告なし（新規に増やさないこと）

- [ ] **Step 5: コミット**

```bash
git add sapphire-journal-server
git commit -m "docs(server): README and a startup log that names the trust model

The server has no TLS and no OAuth, and its keys sit in a plaintext file, so
the private-network assumption belongs where an operator will actually see
it: the first line of the log and the top of the README."
```

---

## 完了後

- framework の Task 1 PR と、この journal の PR は**別々**。framework が `main` に
  入ってから journal の CI が緑になる。
- ledger / desktop への展開はこの計画の範囲外。
- spec の「framework 側の実装から引き継ぐ注意点」に、Windows の代替データストリーム、
  resolver が Mutex を握ったまま呼ばれること、`RemoteBackend` が `snapshot` を
  呼ばないことが書いてある。Task 6 と Task 9 の実装時に読み返すこと。
