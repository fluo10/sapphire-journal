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
