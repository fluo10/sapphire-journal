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

/// 重複の決着に使うのは **journal のキャッシュが id に対応づけているパス**で
/// あって、push した側が名乗った `updated_at` ではないこと。そして負けたほうが
/// 消されずに退避されること。
///
/// 仕掛けは C2 が辿った経路そのもの: リネーム後、古いカーソルのクライアントが
/// 旧パスへ「今」を名乗って push する。旧パスの中身は frontmatter を持たないの
/// で `read_entry` が失敗し、`sync_cache` の `increment_until_free` はこれを
/// 見られない —— `resolve_duplicates` が実際に動く唯一のケース。そのとき
/// `updated_at` で決めると、後から名乗った stale なほうが勝ち、**利用者の
/// 現在のエントリが消える**。
#[tokio::test]
async fn a_stale_push_loses_to_the_live_entry_and_is_quarantined_not_deleted() {
    let h = harness::spawn().await;
    let old_path = h.write_entry_through_mcp("before").await;
    h.tick_once().await;
    let old_rel = h.relative(&old_path);
    let old_name = old_path.file_name().unwrap().to_string_lossy().into_owned();

    let new_path = h.retitle_through_mcp(&old_path, "after").await;
    let new_rel = h.relative(&new_path);

    let push = h
        .rpc(
            "changes.push",
            serde_json::json!({
                "ws": h.ws(), "base_cursor": 0,
                "changes": [{
                    "path": old_rel, "kind": "upsert",
                    "body": "stale edit",
                    // リネームが change log に載った時刻より後。タイムスタンプ
                    // で決めるなら、これだけで stale なほうが勝つ。
                    "updated_at": chrono::Utc::now().to_rfc3339()
                }]
            }),
        )
        .await;
    assert!(
        push["result"]["conflicts"].as_array().is_some_and(|c| c.is_empty()),
        "push は拒否されない想定（LWW で通る）はずが拒否された: {push:?}"
    );
    assert!(
        old_path.exists(),
        "stale push が旧パスにファイルを書き戻していない（テストの前提が崩れている）"
    );

    h.tick_once().await;

    // 1. 生き残ったのはリネーム後のエントリ。パスだけでなく中身で確かめる
    //    —— 逆に決着していたら、ここに "stale edit" が入っている。
    assert!(
        new_path.exists(),
        "利用者の現在のエントリ {new_path:?} が消えた（stale push が勝った）"
    );
    let survived = std::fs::read_to_string(&new_path).unwrap();
    assert!(survived.contains("after"), "生き残ったのが別物: {survived}");
    assert!(!survived.contains("stale edit"), "{survived}");

    // 2. 旧パスは journal から消えている。
    assert!(!old_path.exists(), "旧パスが journal に残っている: {old_path:?}");

    // 3. ただし削除ではなく退避。バグで消えた分は取り返せないが、退避なら
    //    取り返せる。
    let quarantined = h.quarantined();
    assert_eq!(quarantined.len(), 1, "退避されたファイルが 1 件でない: {quarantined:?}");
    let (name, body) = &quarantined[0];
    assert!(
        name.starts_with(&old_name),
        "退避先が元のファイル名を残していない: {name} (元: {old_name})"
    );
    assert!(
        body.contains("stale edit"),
        "退避されたのは負けたほうのファイルではない: {body:?}"
    );

    // 4. change log 側も 1 件に収束し、それがリネーム後のパスであること。
    let snapshot = h.rpc("workspace.snapshot", serde_json::json!({"ws": h.ws()})).await;
    let md: Vec<_> = snapshot["result"]["docs"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["path"].as_str().unwrap().ends_with(".md"))
        .collect();
    assert_eq!(md.len(), 1, "同じ id のエントリが 2 つ残っている: {md:?}");
    assert_eq!(md[0]["path"].as_str().unwrap(), new_rel, "残ったのが旧パスのほう");
}

#[tokio::test]
async fn a_stale_push_to_the_old_path_is_resolved_to_one_entry() {
    let h = harness::spawn().await;
    let path = h.write_entry_through_mcp("before").await;
    h.tick_once().await;
    let old_rel = h.relative(&path);

    // タイトル変更でリネーム。
    h.retitle_through_mcp(&path, "after").await;

    // 古いカーソルのクライアントが旧パスへ push してくる。
    let push = h
        .rpc(
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
    assert!(
        push["result"]["conflicts"].as_array().is_some_and(|c| c.is_empty()),
        "push は拒否されない想定（LWW で通る）はずが拒否された: {push:?}"
    );

    // resolve_duplicates を呼ぶ前に、本当に同じ id のエントリが 2 つ live に
    // なっていることを確認する。ここを確認しないと、stale push がどこかで
    // 弾かれたり期待と違う場所に着地したりしていても、このテストは
    // resolve_duplicates が何もしなくても通ってしまう。
    let before = h.rpc("workspace.snapshot", serde_json::json!({"ws": h.ws()})).await;
    let md_before: Vec<_> = before["result"]["docs"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["path"].as_str().unwrap().ends_with(".md"))
        .collect();
    assert_eq!(
        md_before.len(),
        2,
        "resolve_duplicates を呼ぶ前に 2 つの live エントリが揃っていない（テストの前提が崩れている）: {md_before:?}"
    );

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
