mod harness;

#[tokio::test]
async fn the_tick_picks_up_a_file_written_behind_the_server() {
    let h = harness::spawn().await;

    // 誰も通知してくれない書き込み(外部ツール、手作業)。
    h.write_entry_fixture("2026/gmsdr81_handwritten.md", "gmsdr81", "handwritten", "by hand");

    h.tick_once().await;

    let snapshot = h.rpc("workspace.snapshot", serde_json::json!({"ws": h.ws()})).await;
    let docs = snapshot["result"]["docs"].as_array().unwrap();
    assert!(
        docs.iter().any(|d| d["path"].as_str().unwrap().ends_with("gmsdr81_handwritten.md")),
        "手書きのファイルが回収されていない"
    );
    // この tick が journal のキャッシュも更新していること。change log だけを
    // 見ていると、frontmatter が一切解釈できていなくても通ってしまう。
    assert_eq!(h.cached_title("gmsdr81").as_deref(), Some("handwritten"));
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

/// 同じ状況でも、**押し戻された中身が entry として読める**なら決着をつけるのは
/// `resolve_duplicates` ではない —— tick が先に呼ぶ `sync()` の中で
/// `increment_until_free` が動き、後から現れたほうに新しい id を振って両方残す。
///
/// このテストは以前 `"stale edit"`（frontmatter 無し）を push していたため、
/// `read_entry` が落ちて `increment_until_free` は一度も走っておらず、どちらの
/// 方針も踏んでいなかった。ここでは本物のエントリを押し込んで、id 再発行のほう
/// の経路を通す。
#[tokio::test]
async fn a_stale_push_that_parses_is_reminted_and_leaves_the_live_entry_alone() {
    let h = harness::spawn().await;
    let old_path = h.write_entry_through_mcp("before").await;
    h.tick_once().await;
    let old_rel = h.relative(&old_path);
    // ファイル名の先頭がその id（`{id}_{slug}.md`）。
    let id = old_path
        .file_name()
        .unwrap()
        .to_string_lossy()
        .split('_')
        .next()
        .unwrap()
        .to_owned();

    // タイトル変更でリネーム。
    let new_path = h.retitle_through_mcp(&old_path, "after").await;
    let new_rel = h.relative(&new_path);

    // 古いカーソルのクライアントが旧パスへ push してくる。中身は同じ id を
    // 持つ本物のエントリ（リネーム前に pull したもの、という想定）。
    let push = h
        .rpc(
            "changes.push",
            serde_json::json!({
                "ws": h.ws(), "base_cursor": 0,
                "changes": [{
                    "path": old_rel, "kind": "upsert",
                    "body": harness::render_entry_fixture(&id, "before", "stale edit"),
                    "updated_at": chrono::Utc::now().to_rfc3339()
                }]
            }),
        )
        .await;
    assert!(
        push["result"]["conflicts"].as_array().is_some_and(|c| c.is_empty()),
        "push は拒否されない想定（LWW で通る）はずが拒否された: {push:?}"
    );

    // 決着をつける前に、本当に同じ id のファイルが 2 つ live になっている
    // ことを確認する。ここを確認しないと、stale push がどこかで弾かれたり
    // 期待と違う場所に着地したりしていても、このテストは何もしなくても通る。
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
        "決着をつける前に 2 つの live エントリが揃っていない（テストの前提が崩れている）: {md_before:?}"
    );

    h.tick_once().await;

    // その id を持ち続けるのはリネーム後のエントリ。パスと中身の両方で見る。
    assert_eq!(
        h.cached_title(&id).as_deref(),
        Some("after"),
        "その id が指す先が利用者の現在のエントリでなくなっている"
    );
    assert!(new_path.exists(), "{new_path:?} が消えた");
    let survived = std::fs::read_to_string(&new_path).unwrap();
    assert!(survived.contains("after"), "{survived}");
    assert!(!survived.contains("stale edit"), "{survived}");

    // 押し戻されたほうは消えても退避もされず、**新しい id を振られて残る**。
    assert!(h.quarantined().is_empty(), "読めるエントリが退避されている: {:?}", h.quarantined());
    let snapshot = h.rpc("workspace.snapshot", serde_json::json!({"ws": h.ws()})).await;
    let md: Vec<String> = snapshot["result"]["docs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["path"].as_str().unwrap().to_owned())
        .filter(|p| p.ends_with(".md"))
        .collect();
    assert!(md.contains(&new_rel), "利用者のエントリが同期から消えた: {md:?}");
    assert!(
        !md.contains(&old_rel),
        "旧パスがそのまま残っている（id が振り直されていない）: {md:?}"
    );
    assert_eq!(md.len(), 2, "id を振り直して両方残す想定: {md:?}");
}
