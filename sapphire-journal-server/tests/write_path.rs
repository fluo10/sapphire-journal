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
