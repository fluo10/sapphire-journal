mod harness;

/// この環境は core.autocrlf=true で `.gitattributes` が無いため、チェックアウト
/// した `.md` は CRLF になる。サーバ経由で読むときも壊れないこと。
///
/// 確かめるのは **parse を経た値**（`parser.rs` の CRLF 対応フェンス処理）。
/// change log は本文を不透明なテキストとして持つだけなので、そちらだけを見て
/// いると frontmatter パーサが完全に壊れていても通ってしまう。
#[tokio::test]
async fn a_crlf_entry_round_trips_through_the_server() {
    let h = harness::spawn().await;

    let path = h.write_entry_fixture_crlf("2026/gmsdr80_crlf.md", "gmsdr80", "crlf", "body");
    let raw = std::fs::read_to_string(&path).unwrap();
    assert!(
        raw.contains("\r\n") && !raw.replace("\r\n", "").contains('\n'),
        "フィクスチャが CRLF になっていない（テストの前提が崩れている）: {raw:?}"
    );

    h.tick_once().await;

    assert_eq!(
        h.cached_title("gmsdr80").as_deref(),
        Some("crlf"),
        "CRLF の frontmatter が journal のキャッシュに解釈されて載っていない"
    );

    let snapshot = h.rpc("workspace.snapshot", serde_json::json!({"ws": h.ws()})).await;
    let doc = snapshot["result"]["docs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["path"].as_str().unwrap().ends_with("gmsdr80_crlf.md"))
        .expect("CRLF のエントリが同期に載っていない");
    assert!(doc["body"].as_str().unwrap().contains("body"));
}
