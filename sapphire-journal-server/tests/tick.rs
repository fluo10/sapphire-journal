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
    h.write_entry_through_mcp("note").await;

    h.tick_once().await;
    let report = h.tick_once().await;

    assert_eq!(report.upserted, 0, "変化していないのに再検出している");
    assert_eq!(report.removed, 0);
}
