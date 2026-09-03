# 鍵コマンドをデバイス／ユーザー管理コマンドへ移行する

- 日付: 2026-09-03
- 対象: `sapphire-journal-server/src/{cli,keys,main,serve}.rs`、新規
  `src/cli_device.rs` と `src/device_auth.rs`、`sapphire-journal-server/tests/*`、
  `sapphire-journal-server/README.md`
- 前提: `sapphire-framework` の `sapphire-framework-registry`
  （`Device` / `User` と `.{app_name}/{devices,users}.toml`）と
  `KeyEntry.device_id`
- 関連: issue #279、`sapphire-agent` の
  `2026-08-29-device-based-auth-design.md`（同じ移行を先に済ませている）

## 背景

`sapphire-journal-server` の資格情報は今のところ**鍵しか無い**。`gen-key` が
`keys.toml` に 1 行足し、`protect` がトークンを鍵ファイルと突き合わせて通す。
鍵は誰のものでも何のものでもなく、`label` という自由記述が 1 つ付くだけで、
「どのホストのどのクライアントか」も「誰の持ち物か」も台帳としては存在しない。

framework は既に `sapphire-framework-registry` を持っており、`sapphire-agent` は
2026-08-29 にこの台帳へ移行して、鍵をデバイス行に従属させた。journal-server は
`2026-08-29-framework-key-api-catchup-design.md` の時点で「台帳との連動は入れず
イシューに上げる」と決めており、それが issue #279 になっている。

本 spec はその #279 のうち **CLI と認証**を実装する。フロントマターへの
`updated_by` の焼き込みは含まない（下記「やらないこと」）。

## 決めたこと

1. **`gen-key` / `list-keys` / `rotate-key` / `revoke-key` を削除**し、`device` と
   `user` のサブコマンドに置き換える。互換のための別名は残さない。
2. **台帳を `<root>/.sapphire-journal/{devices,users}.toml` に置く。** 鍵ファイルは
   今まで通りキャッシュディレクトリ（同期されない場所）。
3. **認証をデバイス基準に畳む（fail-closed）。** `device_id` を持たない鍵、台帳に
   無いデバイスを指す鍵、retired なデバイスの鍵は、すべて 401。
4. **既存トークンの救済経路は作らない。** 移行は `device add` のやり直しで行う。

### 1. CLI

```
sapphire-journal-server device add --name <NAME> [--description <TEXT>] [--user <SELECTOR>] [--expires-in <DUR>]
sapphire-journal-server device list
sapphire-journal-server device rotate <SELECTOR> [--expires-in <DUR>]
sapphire-journal-server device retire <SELECTOR> [--purge]

sapphire-journal-server user add --name <NAME> [--description <TEXT>]
sapphire-journal-server user list
```

`sapphire-agent` と同じ形。2 つのリポジトリを行き来する人間が、同じ操作に別の
綴りを覚えずに済むほうが大事なので、綴りを揃えることを優先する。

据え置くもの:

- **出力規約** — トークンだけ stdout、メタデータは stderr。`device add > token.txt`
  が今の `gen-key` と同じように使える。
- **トークン接頭辞** `sjt`。
- **`--expires-in` の書式**（`90d` / `12h` / `30m`、単位必須、0 と負を拒否、範囲外は
  panic ではなくエラー）。`parse_duration` はそのまま使う。
- **`rotate` が期限を保持ではなく置き換える**性質と、そのヘルプでの明示。
- **動いているサーバには再起動まで効かない**という注意書き。`rotate` と `retire`
  の両方に出す。

`device add` の手順は agent と同じ:

1. 失敗しうる解決（`--expires-in` の絶対時刻化、`--user` の解決）を先に済ませる
2. デバイス行を書く
3. `device_id` を入れて鍵を発行する

**行が先、鍵が後**。鍵の無いデバイス行は完全に不活性（誰も認証できない）だが、
逆順で中断すると誰も掃除しない孤児の鍵が残る。その上で `add` を再開可能にする
—— 同名の行が既にあり、このホストの鍵ファイルにその `device_id` の鍵が無ければ、
鍵だけ発行する。鍵もあるならエラーにして `device rotate` を案内する。この分岐が
無いと中断状態から抜ける手段が無い（`rotate` は既存の鍵を要求するため）。

retired な行への `add` と `rotate` は**エラーにする**。retired なデバイスは認証で
必ず弾かれるので、通すと「成功したと言われたのに何も通らないトークン」が出る。
framework には `retired_at` を消す API が無いので、エラー文で `--purge` してから
足し直す道を案内する。

`rotate` と `retire` が鍵を探すときは、`device.name` ではなく **`device_id` で
引く**。`add` は発行時に `label = device.name` を入れるが、`devices.toml` は手編集
され得て、リネームは鍵ファイルの label に伝播しない。名前で引くと、リネーム後の
`retire` が「成功」と言いながら鍵を生かしたまま残す。

### 2. ファイルの置き場所

| ファイル | 場所 | 同期 |
|---|---|---|
| `devices.toml` / `users.toml` | `<root>/.sapphire-journal/` | される |
| `keys.toml` | `Journal::cache_dir()`（従来通り） | されない |

台帳が同期に乗るのは意図的。#279 の狙いは `updated_by: <device_id>` をエントリの
フロントマターに焼くことで、ID を読む側（別のホストのクライアント）が
`device_id → user_id → user.name` を逆引きできなければ意味がない。台帳は content と
一緒に旅する必要がある。秘密は入らない —— トークンは `keys.toml` に留まり、
台帳に載るのは ID・名前・説明・作成日時だけ。

パスは `Journal::journal_dir()`（`<root>/.sapphire-journal`）から組む。framework の
`Workspace::devices_path` と同じ規約だが、journal-server は `Workspace` ではなく
journal-core の `Journal` でルートを解決しているので、そちらから引く。
`serve.rs` に `default_devices_path` / `default_users_path` を置く
（`default_keys_path` の隣）。

#### `--journal-dir` が必須になる

いま `main.rs` の `keys_path_for_key_command` は「`--keys` を明示してあれば
`--journal-dir` は要らない」という逃げ道を持つ。台帳の位置は journal ルートから
しか決まらないので、**`device` と `user` にはこの逃げ道が無い**。解決をコマンド別に
書き直す:

| コマンド | `--journal-dir` | 鍵ファイル |
|---|---|---|
| `user *` | 必須 | 触らない（解決もしない） |
| `device *` | 必須 | `--keys`、無ければ既定 |
| （serve） | 必須 | `--keys`、無ければ既定 |

`--journal-dir` が無いときのエラーは、既存のテスト
（`a_key_command_without_either_explains_itself_in_its_own_terms`）が固定している
方針を引き継ぐ —— そのコマンド自身の言葉で説明し、serve の話にしない。

### 3. 認証

framework の `protect` は `KeyStore` しか見ず、認証結果（`Authenticated`）を
extensions に挿すのは**内側**なので、その値を読むレイヤを外から張ることはできない。
よって journal-server 側で Bearer を自分で読む層を 1 枚持つ。

```rust
pub struct DeviceAuth {
    keys: Arc<KeyStore>,
    devices: Devices,
}

impl DeviceAuth {
    /// トークン → デバイス。失敗理由は区別せず `None` に潰す。
    pub fn resolve(&self, token: &str) -> Option<&Device>;
}
```

`build_router` が返す merge 済み router の**最外周**に被せる。`/rpc` と `/mcp` の
両方が通る唯一の場所で、片方だけ守られている状態を作らない。framework の
`protect` は一次検査としてそのまま残す（二重に鍵を引くが、外した場合に
「レイヤの順序を間違えると素通しになる」構成を自前で抱えることになるほうが悪い）。

401 に潰す条件:

- Bearer ヘッダが無い / 読めない
- トークンが鍵ファイルに無い、または期限切れ
- 鍵に `device_id` が無い
- その `device_id` が台帳に無い
- 台帳の行が retired

区別は `debug!` に出すだけでレスポンスには出さない。「台帳に無い鍵を通す」トグルは
**持たない**。必要になってから足す。

`DeviceAuth` は起動時のスナップショット。`KeyStore` が既にそうであるように、
再読み込みの経路は持たない。`rotate` / `retire` が動いているサーバに効くのは次の
起動時。

#### 起動時ガード

`run_until` の "no usable API key" 検査を、**生きたデバイス行を指す使える鍵が 1 本も
無ければ起動しない**に強める。認証を通れる資格情報が 0 本の状態で待ち受けるのは、
鍵が 0 本のときと同じ事故（誰も繋がらないサーバが黙って上がる）なので、同じ扱いに
する。エラー文は `device add` を案内する。

あわせて、`device_id` を持たない鍵が残っていたら**本数を warn に出す**。
`list-keys` を削除する以上、放置された旧鍵に気づく口はここしか無い。

### 4. 既存トークンを救わない理由

fail-closed にすると、`gen-key` で発行済みのトークンはすべて 401 になる。救済策
（`--adopt-key` で既存の鍵行に `device_id` を後付けする、起動時に label から自動で
デバイス行を作る）は考えたが、採らない:

- 前者は `KeyStore` に in-place 更新の API が無く、framework 側の変更を呼ぶ。
- 後者はサーバが**同期対象のファイルを勝手に書き換える**ことになる。
- どちらも「鍵は必ず台帳を経由する」という不変条件に例外を作る。それが
  この移行で得たいものそのものなので、初日から例外を持つ形で始めない。

移行手順は「`device add` し直してクライアントのトークンを差し替える」。README に
書く。

## やらないこと

- **フロントマターの `updated_by`。** エントリ書き込み時に認証されたデバイスを
  そこまで引き回す配線が要り、本 spec（CLI と認証の移行）とは独立している。
  #279 に残す。
- **`allow_unknown_device` 相当のトグル。**
- **台帳のホットリロード。**

## テスト

**`src/cli_device.rs`（単体、agent と同形）**

- `device add` が行と鍵の両方を書き、鍵の `device_id` が行を指すこと
- `device add` が、鍵の無い既存行を見つけたら鍵だけ発行して完了すること
- 鍵もある同名の行に対しては、`device rotate` を案内してエラーになること
- retired な行への `add` と `rotate` がエラーになること
- `device rotate` が `device_id` で鍵を引き、リネーム後も正しい鍵を差し替えること
- `device retire` が鍵を先に失効させること、`--purge` が行ごと消すこと
- `user add` / `user list`
- 範囲外の `--expires-in` が panic ではなくエラーになること（既存テストの移設）

**`src/cli.rs`（パース）**

- `device add` が `--name` を要求すること、`rotate` / `retire` がセレクタを要求すること
- `user add` が `--name` を要求すること

**`src/main.rs`（パス解決）**

- `user` / `device` が `--journal-dir` 無しで、そのコマンド自身の言葉で失敗すること
- `--keys` を渡しても `device` は `--journal-dir` を要求すること

**結合**

- `tests/harness/mod.rs` を `device add` 経由に差し替える
- 新規 `tests/device_auth.rs`:
  - 生きたデバイスのトークンで `/rpc` と `/mcp` の両方が通る
  - `device_id` 無しのトークン（手書きの `keys.toml`）が 401
  - 台帳に無い `device_id` を指すトークンが 401
  - retired なデバイスのトークンが 401（再起動＝状態の作り直し後）
- `tests/no_keys.rs` を新しい起動条件に拡張する。鍵はあるがデバイス行が無い、
  という状態でも起動を拒否すること
- `tests/revoke.rs` を `device retire` に読み替える

## ドキュメント

- `sapphire-journal-server/README.md` の `gen-key` 手順を `user add` →
  `device add` に差し替える。認証の節（デバイスが主体であること、401 になる条件）と、
  既存トークンが失効することと移行手順を書く。鍵ファイルが空のときの案内
  （現在は `gen-key` を指している）も直す
- `sapphire-journal-server` に CHANGELOG.md は無い（`publish = false`）。破壊的変更は
  README の移行手順とコミットメッセージで示す
