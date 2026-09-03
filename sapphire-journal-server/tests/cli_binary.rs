//! バイナリを**本当に起動して**、サブコマンドが最後まで通ることを見る。
//!
//! ライブラリ側のテストはどれも自分で `init_app_context()` を呼ぶので、
//! 「`main` がそれを呼び忘れている」という欠陥だけはライブラリ経由では一度も
//! 踏めない —— `Journal::cache_dir()` が panic するのは、初期化していない
//! プロセスの中だけだから。ここだけが本物の `main` を通る。
//!
//! ついでに出力規約（トークンや id だけ stdout、メタデータは stderr）も、
//! 実プロセスの stdout と stderr を分けて確かめる。ここ以外にこの契約を
//! 端から端まで見ている場所は無い。

use std::path::Path;
use std::process::{Command, Output};

/// `.sapphire-journal/` を掘って journal にする。台帳と鍵の解決に必要なのは
/// マーカーディレクトリだけなので、config は空でよい。
fn init_journal(root: &Path) {
    std::fs::create_dir_all(root.join(".sapphire-journal")).unwrap();
    std::fs::write(root.join(".sapphire-journal").join("config.toml"), "").unwrap();
}

/// バイナリを起動する。
///
/// **`--keys` は渡さない。** 渡すと鍵ファイルの位置が確定してキャッシュ
/// ディレクトリを引かずに済んでしまい、このテストが見たい経路
/// （`Journal::cache_dir()` → `AppContext`）を通らなくなる。
///
/// 代わりにキャッシュの置き場所を環境変数で一時ディレクトリへ逃がす。
/// Linux は `XDG_CACHE_HOME`、macOS は `HOME` 経由で `dirs` が拾う。Windows の
/// `dirs::cache_dir` は環境変数ではなく known folder API を見るので、そこでは
/// ランナーの本物のキャッシュ配下に鍵ファイルが 1 つできる（一時 journal の
/// パス uuid ごとのサブディレクトリなので、他と混ざりはしない）。
fn run(journal_dir: &Path, cache_home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_sapphire-journal-server"))
        .env("XDG_CACHE_HOME", cache_home)
        .env("XDG_DATA_HOME", cache_home.join("data"))
        .env("HOME", cache_home)
        .arg("--journal-dir")
        .arg(journal_dir)
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn device_add_runs_end_to_end_and_prints_only_the_token_on_stdout() {
    let tmp = tempfile::tempdir().unwrap();
    let journal_dir = tmp.path().join("journal");
    init_journal(&journal_dir);

    let out = run(&journal_dir, &tmp.path().join("cache"), &["device", "add", "--name", "laptop"]);

    let stdout = String::from_utf8(out.stdout).unwrap();
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(out.status.success(), "device add が失敗した\nstderr: {stderr}");
    // stdout はトークン 1 行だけ。`device add > token.txt` がそのまま使える、
    // という README の約束はこの 1 行が守っている。
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 1, "stdout が 1 行ではない: {stdout:?}");
    assert!(lines[0].starts_with("sjt_"), "トークンではない: {:?}", lines[0]);
    // メタデータは stderr。
    assert!(stderr.contains("created"), "メタデータが stderr に出ていない: {stderr}");
    assert!(!stdout.contains("created"), "メタデータが stdout に混ざっている: {stdout}");

    // 台帳が journal の中に、鍵がその外にできていること。
    assert!(
        journal_dir.join(".sapphire-journal").join("devices.toml").exists(),
        "デバイス台帳ができていない"
    );
    assert!(
        !journal_dir.join(".sapphire-journal").join("keys.toml").exists(),
        "鍵ファイルが同期される場所にできている"
    );
}

#[test]
fn user_add_runs_end_to_end_and_prints_only_the_id_on_stdout() {
    let tmp = tempfile::tempdir().unwrap();
    let journal_dir = tmp.path().join("journal");
    init_journal(&journal_dir);

    let out = run(&journal_dir, &tmp.path().join("cache"), &["user", "add", "--name", "fluo"]);

    let stdout = String::from_utf8(out.stdout).unwrap();
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(out.status.success(), "user add が失敗した\nstderr: {stderr}");
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 1, "stdout が 1 行ではない: {stdout:?}");
    assert!(stderr.contains("added"), "メタデータが stderr に出ていない: {stderr}");
    assert!(
        journal_dir.join(".sapphire-journal").join("users.toml").exists(),
        "ユーザー台帳ができていない"
    );
}
