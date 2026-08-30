# framework の鍵 API 追従と `rotate-key`

- 日付: 2026-08-29
- 対象: `sapphire-journal-server/src/{cli,keys}.rs`, `Cargo.lock`
- 前提: `sapphire-framework` の `2026-08-29-api-key-rotation-design.md` と
  `2026-08-29-device-user-registry-design.md`

## 背景

`sapphire-journal-server` は `sapphire-framework` を `5918b03`（PR #105）にピン留めしている。
その後 framework 側で鍵 API が変わった：

- `KeyEntry::id` が grain-id を経由して **UUID に戻った**
- `KeyStore::generate` が **`id: Option<Uuid>` を受け取るようになり 4 引数になった**
- `KeyStore::rotate` が新設された（id・label・`created_at` を保ったまま token だけ差し替える）
- `KeyEntry` に `rotated_at` が増えた
- （registry の spec で）`KeyEntry` に `device_id: Option<GrainId>` が増える

ピンを `main` へ上げると、`src/keys.rs` の `store.generate(TOKEN_PREFIX, label, expires_at)` が
**素直にコンパイルエラーになる**。

## 決めたこと

1. **ピンを `main` へ上げ、`generate` の呼び出しを 4 引数に直す。** journal-server は鍵の id を
   自分で決める理由が無いので `None` を渡す。
2. **`rotate-key` サブコマンドを足す。** `gen-key` / `list-keys` / `revoke-key` があって
   再発行だけ穴が空いている状態を埋める。
3. **`list-keys` の行に `rotated_at` を出す。**
4. **デバイス／ユーザー台帳との連動は入れない。** イシューに上げるだけ。

### 2. `rotate-key`

```
sapphire-journal-server rotate-key <SELECTOR> [--expires-in <DURATION>]
```

`gen-key` と同じ出力規約 — トークンだけ stdout、メタデータは stderr。

`expires_at` が**保持ではなく置き換え**である点は framework 側のドキュメントの通り。期限切れの鍵を
期限そのままで再発行しても使えないので、呼び出し側に指定させる。`--expires-in` を省くと無期限に
なる（既存の期限は引き継がれない）。ヘルプにこれを明記する。

旧トークンは即座に無効になるが、「即座」はプロセス内の話。`ServerState` は起動時に取った
`Arc<KeyStore>` のスナップショットを持つだけで再読み込みの経路が無いため、**動いているサーバに
効くのは次にファイルを読み直したとき（再起動時）**。`revoke-key` が既に同じ性質を持っており、
`tests/revoke.rs` がそれを固定している。`rotate-key` のヘルプにも同じ注意を書く。

### 3. `list-keys` に `rotated_at`

`format_key_line` は現在 id・トークン（マスク済）・`created_at`・期限・label を出す。`rotated_at` を
足す。一度も再発行していない鍵では `None` なので `-` を出す。

### 4. 台帳との連動を入れない理由

将来的にはユーザーリストとデバイスリストを連動させ、デバイスをユーザーに紐づけて、ジャーナルの
フロントマターに `updated_by` として最終更新者を焼き、表示時に `device_id → device.user_id →
user.name` と逆引きしたい。framework の registry はそのために用意される。

ただしそれは journal-server 側に「エントリ書き込み時に認証されたデバイスを引き回す」という別の
配線を要求する変更で、本 spec の目的（コンパイルを通す）とは独立している。イシューに上げる。

## テスト

既存の `src/keys.rs` のテスト群と同じ形で：

- `rotate-key` が token を差し替え、id・label・`created_at` を保つこと
- `rotate-key` が `rotated_at` を立てること
- `--expires-in` 無しの `rotate-key` が既存の期限を引き継がず無期限にすること（**意図した挙動**で
  あることをテストで固定する）
- 存在しないセレクタでエラーになること
- `list-keys` が `rotated_at` を出し、未再発行の鍵では `-` になること
- CLI パース（`src/cli.rs` の既存テストの形）

`tests/revoke.rs` と同じ形で、rotate 後に旧トークンが（再読み込み後に）弾かれることを見る
結合テストを足すかは実装時に判断する。

## 立てるイシュー

**デバイス／ユーザー台帳と `updated_by`。** framework の `sapphire-framework-registry` を使って
`.sapphire-journal/{devices,users}.toml` を持ち、`KeyEntry.device_id` から書き込み元デバイスを
解決して、エントリのフロントマターに `updated_by: <device_id>` を焼く。表示時は
`device_id → user.name` で逆引きし、最終更新者が人間か AI かを区別する。`sapphire-agent` は
MCP クライアントとして 1 つのデバイスとして台帳に載る。
