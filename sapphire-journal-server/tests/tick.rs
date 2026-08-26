mod harness;

#[tokio::test]
async fn the_tick_picks_up_a_file_written_behind_the_server() {
    let h = harness::spawn().await;

    // 誰も通知してくれない書き込み(外部ツール、手作業)。
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

    // 起動直後の 1 回に相当する呼び出し。track db が空のままだと
    // `.sapphire-journal/config.toml`(journal 初期化時から存在し、
    // is_syncable の対象)まで「初めて見る」扱いになり、これから確認したい
    // MCP 書き込みの検出数に紛れ込む。実サーバでも `spawn_tick` は起動直後に
    // 1 回走る (`tokio::time::interval` の最初の `tick()` は待たずに返る) の
    // で、ここで先に 1 回走らせておくのは実際の起動シーケンスと同じ。
    h.tick_once().await;

    h.write_entry_through_mcp("note").await;

    // MCP の書き込みは change log には載る (Task 7 のオブザーバ) が、track db
    // には載らない — それを更新するのは reconcile だけ。だから次の tick は
    // 「初めて見る」差分として検出するはず。ここを確認しないと、tick_once を
    // 丸ごと no-op にしても 0 == 0 のまま通ってしまい、このテストが証明したい
    // はずの「2 回目が静かなのは冪等だから」を何も検証できない。
    let first = h.tick_once().await;
    assert_eq!(first.upserted, 1, "MCP で書いた 1 件を検出するはず");
    assert_eq!(first.removed, 0);

    let second = h.tick_once().await;
    assert_eq!(second.upserted, 0, "変化していないのに再検出している");
    assert_eq!(second.removed, 0);
}
