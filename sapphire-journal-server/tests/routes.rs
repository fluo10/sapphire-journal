//! `/rpc` と `/mcp` が同じ Router に載り、同じ鍵で守られていることを確かめる。

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use tower::ServiceExt as _;

mod harness;

#[tokio::test]
async fn both_routes_reject_a_request_without_a_token() {
    let h = harness::spawn().await;

    for uri in ["/rpc", "/mcp"] {
        let response = h
            .router()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "{uri} が無認証で通っている"
        );
    }
}

#[tokio::test]
async fn rpc_answers_a_snapshot_with_the_key() {
    let h = harness::spawn().await;

    let response = h.rpc("workspace.snapshot", serde_json::json!({"ws": h.ws()})).await;

    assert!(response.get("error").is_none(), "{response:?}");
    let result = &response["result"];
    assert!(result.get("generation").is_some(), "世代 ID が返っていない");
}
