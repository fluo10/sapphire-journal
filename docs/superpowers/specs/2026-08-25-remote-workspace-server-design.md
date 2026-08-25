# sapphire-journal リモートワークスペースサーバ

- 日付: 2026-08-25
- 対象: 新規クレート `sapphire-journal-server`、既存 `sapphire-journal-mcp` / `-core`
- 前提: `sapphire-framework` の `2026-08-25-app-service-coexistence-design.md`
  （本 spec はそこで追加される API を使う）

## 目的

**人間と AI が同じ journal を共同編集できるようにする。**

- 人間は手元のクライアント（CLI / desktop）で編集し、framework の差分同期でサーバと収束する。
- AI は sapphire-agent 等から、サーバの MCP エンドポイントを叩いて編集する。
- 同一エントリの同時編集（文字単位のマージ）は要求しない。**別々のエントリを並行編集できれば十分。**

エントリは `{year}/{id}_{slug}.md` で **1 エントリ 1 ファイル**（`journal.rs` の
`new_entry_path` / `entry_filename`）なので、framework のパス単位 LWW がそのまま
「別レコードは素通し、同一レコードは LWW」という要求粒度に一致する。

## なぜ新クレートか

`sapphire-journal-cli` の `serve` サブコマンドではなく、独立した `sapphire-journal-server` を作る。

1. **デプロイ先が違う**。CLI は各自のマシンで `entry add` を叩く軽いバイナリ（現状 `tokio` は
   `rt` のみ）。サーバは常駐ホスト。
2. **feature の伸びる向きが逆**。framework の ARCHITECTURE では semantic 検索の
   「server embedder は後続」とされており、サーバ側は将来 embedder を抱える方向、
   CLI は軽くしたい方向。cargo の feature は加算的なので同居させると両方が太る。
3. **リリースが独立する**。`release-plz.toml` は crate ごとにタグを切っている。
   サーバの変更で CLI のバージョンを上げたくない。
4. **既存パターンと揃う**。`sapphire-journal-mcp` が lib、CLI が薄い入口、という形が既にある。

依存は**ファサード crate 経由**にする — `sapphire-framework = { features = ["remote-server"] }`
を使い、`sapphire_framework::remote_server::{...}` を呼ぶ。`sapphire-timer-server` が既に
この形（サブクレートを直接参照していない）。

`sapphire-journal-server` は **lib + bin**。lib は `Router` を組み立てて返し、bin は設定を
読んで listen するだけ。lib を分けておくと desktop に埋め込む余地が残る。

**先例がある**: `sapphire-timer-server` が既に独立 crate として存在し、framework の
remote-server をそのまま起動している（ただし MCP は載せていない）。本 spec はその形に
MCP エンドポイントと書き込み経路を足したもの。ledger も後追いで同じ形になる。
**プロセスは別、ポートも別**。
各アプリのサーバが固有の MCP エンドポイントを持つため、1 プロセスへの相乗りはしない。

## 構成

```
sapphire-journal-server（1 プロセス / 1 ポート / 1 axum アプリ）
  ├ POST /rpc  ← sapphire-framework-remote-server::router()
  └      /mcp  ← sapphire-journal-mcp（http-server feature）を protect() で保護
  どちらも同じ journal ルートディレクトリを見る
```

- origin = **実在の journal ルート**（`.sapphire-journal/` を含む）。
  framework 側の `WsStoreConfig` に、この origin と journal 自身の retrieve ストアを注入する。
  そうしないと同じファイル群にインデックスが 2 つできる。
- change log / track db / blob は `Journal::cache_dir()` 配下、すなわち
  `~/.cache/sapphire-journal/{uuid}/`（プラットフォーム毎のアプリキャッシュ）に置く。
  ここは journal ルートの外で、`journal.rs` の doc コメントが明記している通り
  git / Syncthing / Nextcloud に同期されない。
- `.sapphire-journal/config.toml` はワークスペースレベルの設定なので、**同期されるのが
  正しい**（端末間で共有したい）。これが `app_dir` を名指しする理由。
- **同期対象は許可制。** 隠しファイル・隠しディレクトリは原則すべて除外し、
  `WsStoreConfig.app_dir` に `".sapphire-journal"` を指定して**そこだけ**通す。
  journal ルートを外部ツールで git 管理していても `.git/` は自然に外れる。

`sapphire-journal-mcp` の `serve_http` は Router を内部で組んで自分でリッスンしてしまうため
（`http.rs`）、**Router（`StreamableHttpService`）を返すアクセサを追加**する。
`serve_http` はそれを使う薄いラッパとして残し、desktop の既存利用を壊さない。

## 書き込み経路

MCP のツールハンドラは今まで通り `journal-core::ops` でファイルを書く。**書いた直後に
`record_local_write` を呼ぶ薄いラッパ**を `sapphire-journal-server` 側に置く。
`sapphire-journal-mcp` 本体（stdio で使う版）は変更しない。

### リネームを 1 バッチで扱う

`ops` はタイトルやスラグが変わるとファイル名を `{id}_{slug}.md` に合わせて `std::fs::rename`
する。つまり 1 回の編集が「旧パスの delete ＋ 新パスの upsert」という **2 パスの変更**になる。

都合のよいことに、`ops::update_entry` と `ops::fix_entry` は**リネームしたときだけ
`Some(new_path)` を返す**（`ops.rs`）。ラッパは呼び出し前のパスとこの戻り値を見るだけで、
記録すべきパス集合を正確に組み立てられる。

これを 1 回の `record_local_write` 呼び出しにまとめる。分けて記録すると、pull した側が
一瞬エントリを失ったり、二重に見えたりする。

### 旧パスへの push を弾く（id ガード）

古いカーソルのクライアントが、リネーム前のパスへの編集を push してくる場合がある。
パス単位 LWW では**旧パスが復活し、同じエントリが 2 ファイルになる**。

ファイル名の stem に `GrainId` が入っているので、journal-server 側で
「同じ id を持つ別パスが既に live なら、旧パスへの upsert を拒否して `conflicts` に載せる」
ガードを入れる。クライアントは `conflicts` を見て pull し直す。

framework 層はパスだけを見る汎用のままにし、**id を知っているガードは journal-server に置く**。

## 走査の一本化

サーバ上では放っておくと走査が 3 つ並ぶ。

1. journal MCP の定期再インデックス（`server.rs` の `spawn_periodic_reindex`、
   `UserConfig::sync_interval()` 由来）
2. journal 自身の entries/tags キャッシュ同期（`cache.rs` の `sync_cache`）
3. framework の整合スキャン（`WsStore::reconcile`）

サーバ構成では **1 つのティックに統合**する。`track` で 1 回走査し、その差分から
journal キャッシュ更新と change log 追記の両方を行う。MCP の定期再インデックスは
サーバ構成では起動しない（`spawn_periodic_reindex` を呼ばない）。

## 認証と鍵管理

- `/rpc` と `/mcp` の**両方**に、framework の同じ鍵で認証をかける。`/mcp` は framework の
  `protect()` を通す。
- rmcp は既定で `Host` ヘッダをループバックに制限している（`http.rs` の注記）。bind アドレスに
  合わせて許可リストを広げるが、**認証層とセットでしか触らない**。片方だけ緩めると素通しになる。
- トークンのプレフィクスは journal では **`sjt_`**（sapphire-journal token）。API キーの
  一般的な慣習（Synapse `syt_`、GitHub `ghp_`、Stripe `sk_live_`、Anthropic `sk-ant-`）に
  倣い、略語＋用途の 1 文字とする。`sj` 単体だとアプリ名なのかトークンなのか読み取れない。
  他アプリも機械的に `slt_` / `stt_` / `sat_` と伸ばせる。
- TLS / OAuth は範囲外。**プライベート網（VPN / Tailscale / LAN）前提**であることを
  README と起動ログに明記する。

### サブコマンド

`serve` は既定動作なのでサブコマンド無しで起動する（clap の `Option<Subcommand>`、
`None` なら serve）。既存の `sapphire-timer-server` もサブコマンド無しの serve のみで、
これに揃う。

引数の形も timer-server に合わせる: `--addr` は `SocketAddr` を 1 本で受け（`--bind` と
`--port` に分けない）、環境変数は `SAPPHIRE_JOURNAL_SERVER_*` を使う。

```
sapphire-journal-server [--journal-dir …] [--addr …] [--keys …]
sapphire-journal-server gen-key ["label"] [--expires-in 90d]
sapphire-journal-server list-keys
sapphire-journal-server revoke-key <uuid|label>
```

- `gen-key` のラベルは任意（位置引数）。`--expires-in` は**生成時に絶対時刻へ変換**して
  `expires_at` に保存する。ファイルには絶対時刻だけを持たせるほうが、後から読んだときに
  曖昧さがない。
- `list-keys` は id・label・作成日時・期限（期限切れは `expired` 表示）を出す。トークンはマスク。
- 鍵ファイルの読み書きと検証の実体は framework 側。ここはそれを呼ぶだけ。

### 鍵 UUID とユーザーの紐づけ（将来）

認証 layer はリクエスト拡張に `Authenticated { key_id, label }` を載せる。**今回はここまで。**

将来、journal の最終更新ユーザーを識別したくなったら、ワークスペース内にユーザー一覧
（`.sapphire-journal/users.toml` 相当）を置き、鍵の `key_id` からユーザーを引く。
label を変えても紐づけは切れない。**ユーザー一覧の設計そのものは今回のスコープ外。**

## 設定

サーバ設定は journal ルートの外（サーバ設定ファイル）に置く。`--keys` の既定値はその隣。
`journal_dir` は上位探索をせず、明示指定されたディレクトリを直接開く
（`serve_http` が既にそうしている理由と同じ。サーバはどの journal を公開するか知っている）。

## テスト

- `/rpc` と `/mcp` を同一 Router に載せた統合テスト（`tower::ServiceExt::oneshot`）。
  **MCP でエントリを作る → `changes.pull` に出る**、を 1 本で通す。
- タイトル変更によるリネームが 1 バッチで記録され、pull 側で一貫して見えること。
- 旧パスへの push が `conflicts` に載って拒否されること（id ガード）。
- 認証: トークン無しで `/mcp` が 401 になること。`/rpc` だけ守られている退行を防ぐ。
- `gen-key` → その鍵で `/rpc` と `/mcp` の両方が通ること。`revoke-key` 後に 401 になること。
- **CRLF**: サーバ経由で書いたエントリと、CRLF で置かれた既存エントリの双方が
  正しく読めること（既存の教訓に従い 1 本入れる）。

## 段階

1. **framework PR**（別 spec）: `WsStoreConfig` / `record_local_write` / `reconcile` /
   世代 ID / `KeyStore` / 認証 layer。単体で緑にする。
2. **journal PR**（本 spec）: `sapphire-journal-server` 新設、mcp の Router アクセサ追加、
   Router 合成、ops ラッパと id ガード、走査統合、鍵サブコマンド、
   `release-plz.toml` に `server-v*` を追加。
3. ledger・timer は 2 と同じ形をなぞる（**今回は対象外**）。

## スコープ外

- 書き込みごとの作成者記録、ユーザー一覧設定
- 鍵ごとの権限差（読み取り専用鍵など）
- TLS / OAuth / インターネット公開
- 同一エントリの文字単位マージ（CRDT）
- ledger・timer への展開
