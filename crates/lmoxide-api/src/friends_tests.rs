//! Account friends through the whole router, and the bot's DM queue; `/v1/version`.

use axum::http::StatusCode;
use serde_json::json;

use crate::discord_tests::Api;

#[tokio::test]
async fn account_friends() {
    let api = Api::new(&["--registration", "discord"]);
    let alice = api.db.create_account("alice", Some("password1"), 0).unwrap();
    let (_, bob) = api.register("100000000000000002", "bob", 0).await;
    let bob_key = bob["key"]["secret"].as_str().unwrap().to_string();
    let bob_id = bob["link"]["account_id"].as_u64().unwrap() as u32;
    let alice_s = api.db.create_session(alice.id, 3600).unwrap();

    // a key that isn't an account's has no friends
    assert_eq!(api.call("GET", "/v1/me/friends", Some(&api.bot), None).await.0, StatusCode::NOT_FOUND);
    assert_eq!(api.call("POST", "/v1/me/friends", Some(&alice_s), Some(json!({"username": "nobody"}))).await.0, StatusCode::NOT_FOUND);
    assert_eq!(api.call("POST", "/v1/me/friends", Some(&alice_s), Some(json!({"username": "alice"}))).await.0, StatusCode::UNPROCESSABLE_ENTITY);

    let (s, f) = api.call("POST", "/v1/me/friends", Some(&alice_s), Some(json!({"username": "BOB"}))).await;
    assert_eq!((s, &f["state"], &f["username"]), (StatusCode::CREATED, &json!("outgoing"), &json!("bob")));
    assert_eq!(api.call("POST", "/v1/me/friends", Some(&alice_s), Some(json!({"username": "bob"}))).await.0, StatusCode::CONFLICT);

    // the bot: bob is linked, so his request waits for a DM, once; players can't see the queue
    assert_eq!(api.call("GET", "/v1/discord/friend-requests", Some(&bob_key), None).await.0, StatusCode::FORBIDDEN);
    let (s, q) = api.call("GET", "/v1/discord/friend-requests", Some(&api.bot), None).await;
    assert_eq!((s, &q[0]["requester_username"], &q[0]["discord_id"]), (StatusCode::OK, &json!("alice"), &json!("100000000000000002")));
    let notified = format!("/v1/discord/friend-requests/{}/{bob_id}/notified", alice.id);
    assert_eq!(api.call("POST", &notified, Some(&api.bot), None).await.0, StatusCode::NO_CONTENT);
    assert_eq!(api.call("GET", "/v1/discord/friend-requests", Some(&api.bot), None).await.1, json!([]));
    assert_eq!(api.call("POST", &notified, Some(&api.bot), None).await.0, StatusCode::NO_CONTENT, "again: still fine");

    // bob answers with his account key (the bot's buttons)
    let (_, list) = api.call("GET", "/v1/me/friends", Some(&bob_key), None).await;
    assert_eq!((&list[0]["state"], &list[0]["account_id"]), (&json!("incoming"), &json!(alice.id)));
    assert_eq!(api.call("POST", &format!("/v1/me/friends/{bob_id}/accept"), Some(&alice_s), None).await.0, StatusCode::NOT_FOUND, "only the target accepts");
    let (s, f) = api.call("POST", &format!("/v1/me/friends/{}/accept", alice.id), Some(&bob_key), None).await;
    assert_eq!((s, &f["state"]), (StatusCode::OK, &json!("friends")));
    assert_eq!(api.call("GET", "/v1/me/friends", Some(&alice_s), None).await.1[0]["state"], json!("friends"));

    // removing; a rejected request is gone
    assert_eq!(api.call("DELETE", &format!("/v1/me/friends/{bob_id}"), Some(&alice_s), None).await.0, StatusCode::NO_CONTENT);
    assert_eq!(api.call("GET", "/v1/me/friends", Some(&bob_key), None).await.1, json!([]));
    api.call("POST", "/v1/me/friends", Some(&bob_key), Some(json!({"username": "alice"}))).await;
    assert_eq!(api.call("POST", &format!("/v1/me/friends/{bob_id}/reject"), Some(&alice_s), None).await.0, StatusCode::NO_CONTENT);
    assert_eq!(api.call("GET", "/v1/me/friends", Some(&alice_s), None).await.1, json!([]));
}

#[tokio::test]
async fn version_without_a_game_server() {
    let api = Api::new(&[]);
    let (s, v) = api.call("GET", "/v1/version", None, None).await;
    assert_eq!((s, &v["api"]["version"], &v["game_server"]), (StatusCode::OK, &json!(env!("CARGO_PKG_VERSION")), &json!(null)));
    let api = Api::new(&["--private-status"]);
    assert_eq!(api.call("GET", "/v1/version", None, None).await.0, StatusCode::UNAUTHORIZED);
}
