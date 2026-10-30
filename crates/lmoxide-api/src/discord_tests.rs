//! The Discord routes through the whole router: registration modes, account keys, codes.

use std::{net::SocketAddr, sync::Arc};

use axum::{
    Router,
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use clap::Parser;
use http_body_util::BodyExt;
use lmoxide_db::{Db, Scope};
use serde_json::{Value, json};
use tower::ServiceExt;

#[derive(Parser)]
struct Args {
    #[command(flatten)]
    cfg: crate::Config,
}

struct Api {
    app: Router,
    db: Arc<Db>,
    bot: String,
}

impl Api {
    fn new(args: &[&str]) -> Self {
        let cfg = Args::parse_from(std::iter::once("api").chain(args.iter().copied())).cfg;
        let db = Arc::new(Db::open_in_memory().unwrap());
        let (_, bot) = db.create_api_key("discord-bot", Scope::Discord).unwrap();
        Api { app: crate::app(cfg, db.clone()).unwrap(), db, bot }
    }

    async fn call(&self, method: &str, path: &str, token: Option<&str>, body: Option<Value>) -> (StatusCode, Value) {
        let mut req = Request::builder().method(method).uri(path);
        if let Some(t) = token {
            req = req.header("authorization", format!("Bearer {t}"));
        }
        let mut req = match body {
            Some(b) => req.header("content-type", "application/json").body(Body::from(b.to_string())),
            None => req.body(Body::empty()),
        }
        .unwrap();
        req.extensions_mut().insert(ConnectInfo(SocketAddr::from(([192, 0, 2, 1], 1234))));
        let res = self.app.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    async fn register(&self, discord_id: &str, username: &str, gm_level: u32) -> (StatusCode, Value) {
        let body = json!({"discord_id": discord_id, "username": username, "password": "password1", "gm_level": gm_level});
        self.call("POST", "/v1/discord/accounts", Some(&self.bot), Some(body)).await
    }
}

#[tokio::test]
async fn registration_modes() {
    let creds = json!({"username": "someone", "password": "password1"});
    let api = Api::new(&["--registration", "discord"]);
    let (s, cfg) = api.call("GET", "/v1/config", None, None).await;
    assert_eq!((s, &cfg["registration"], &cfg["registration_open"]), (StatusCode::OK, &json!("discord"), &json!(false)));
    let (s, e) = api.call("POST", "/v1/auth/register", None, Some(creds.clone())).await;
    assert_eq!((s, &e["error"]["code"]), (StatusCode::FORBIDDEN, &json!("registration_closed")));
    assert_eq!(api.register("1", "viaDiscord", 0).await.0, StatusCode::CREATED);

    let api = Api::new(&[]);
    assert_eq!(api.call("GET", "/v1/config", None, None).await.1["registration"], json!("closed"));
    assert_eq!(api.register("1", "viaDiscord", 0).await.0, StatusCode::FORBIDDEN);

    let api = Api::new(&["--open-registration"]);
    let (_, cfg) = api.call("GET", "/v1/config", None, None).await;
    assert_eq!((&cfg["registration"], &cfg["registration_open"]), (&json!("open"), &json!(true)));
    assert_eq!(api.call("POST", "/v1/auth/register", None, Some(creds)).await.0, StatusCode::CREATED);
    assert_eq!(api.register("1", "viaDiscord", 0).await.0, StatusCode::CREATED);
}

#[tokio::test]
async fn account_keys_act_as_the_account() {
    let api = Api::new(&["--registration", "discord"]);
    let (s, r) = api.register("165540221100752897", "alice", 0).await;
    assert_eq!(s, StatusCode::CREATED);
    assert_eq!(r["link"]["discord_id"], json!("165540221100752897"));
    assert_eq!(r["key"]["key"]["account_id"], r["link"]["account_id"]);
    let key = r["key"]["secret"].as_str().unwrap().to_string();

    // the same Discord account or username again
    assert_eq!(api.register("165540221100752897", "alice2", 0).await.0, StatusCode::CONFLICT);
    assert_eq!(api.register("2", "ALICE", 0).await.0, StatusCode::CONFLICT);
    assert!(api.db.account_by_name("alice2").unwrap().is_none());
    assert_eq!(api.register("12a", "carol", 0).await.0, StatusCode::UNPROCESSABLE_ENTITY);

    // a player's key: only /v1/me
    let (s, me) = api.call("GET", "/v1/me", Some(&key), None).await;
    assert_eq!((s, &me["username"], &me["discord_id"]), (StatusCode::OK, &json!("alice"), &json!("165540221100752897")));
    assert_eq!(api.call("GET", "/v1/accounts", Some(&key), None).await.0, StatusCode::FORBIDDEN);
    assert_eq!(api.call("GET", "/v1/discord/links", Some(&key), None).await.0, StatusCode::FORBIDDEN);

    // the GM level decides its scope
    let (s, l) = api.call("PATCH", "/v1/discord/links/165540221100752897", Some(&api.bot), Some(json!({"gm_level": 1}))).await;
    assert_eq!((s, &l["gm_level"]), (StatusCode::OK, &json!(1)));
    assert_eq!(api.call("GET", "/v1/accounts", Some(&key), None).await.0, StatusCode::OK);
    assert_eq!(api.call("GET", "/v1/api-keys", Some(&key), None).await.0, StatusCode::FORBIDDEN);
    api.call("PATCH", "/v1/discord/links/165540221100752897", Some(&api.bot), Some(json!({"gm_level": 2}))).await;
    assert_eq!(api.call("GET", "/v1/api-keys", Some(&key), None).await.0, StatusCode::OK);
    let (_, audit) = api.call("GET", "/v1/audit", Some(&key), None).await;
    assert!(audit.as_array().unwrap().iter().any(|e| e["actor"] == json!("api-key:discord-bot") && e["action"] == json!("account-gm")));

    // the bot's key can't use admin routes, and the PATCH only reaches linked accounts
    assert_eq!(api.call("GET", "/v1/api-keys", Some(&api.bot), None).await.0, StatusCode::FORBIDDEN);
    assert_eq!(api.call("GET", "/v1/accounts", Some(&api.bot), None).await.0, StatusCode::OK);
    let (s, _) = api.call("PATCH", "/v1/discord/links/99", Some(&api.bot), Some(json!({"gm_level": 2}))).await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    // banned: the key stops working
    let id = api.db.account_by_name("alice").unwrap().unwrap().id;
    api.db.ban(id, &lmoxide_db::Ban { until: None, reason: None }).unwrap();
    assert_eq!(api.call("GET", "/v1/me", Some(&key), None).await.0, StatusCode::UNAUTHORIZED);
    let (_, l) = api.call("GET", "/v1/discord/links/165540221100752897", Some(&api.bot), None).await;
    assert_eq!(l["banned"], json!(true));
    api.db.unban(id).unwrap();

    // a new key replaces the old one; unlinking revokes it
    let (s, k) = api.call("POST", "/v1/discord/links/165540221100752897/api-key", Some(&api.bot), None).await;
    assert_eq!(s, StatusCode::CREATED);
    let key2 = k["secret"].as_str().unwrap().to_string();
    assert_eq!(api.call("GET", "/v1/me", Some(&key), None).await.0, StatusCode::UNAUTHORIZED);
    let (_, me) = api.call("GET", "/v1/me", Some(&key2), None).await;
    assert_eq!(me["username"], json!("alice"));
    assert_eq!(api.call("GET", "/v1/discord/links", Some(&api.bot), None).await.1.as_array().unwrap().len(), 1);
    assert_eq!(api.call("DELETE", "/v1/discord/links/165540221100752897", Some(&api.bot), None).await.0, StatusCode::NO_CONTENT);
    assert_eq!(api.call("GET", "/v1/me", Some(&key2), None).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(api.call("GET", "/v1/discord/links/165540221100752897", Some(&api.bot), None).await.0, StatusCode::NOT_FOUND);
    assert!(api.db.account(id).unwrap().is_some(), "the account stays");
}

#[tokio::test]
async fn link_and_login_codes() {
    let api = Api::new(&[]);
    api.db.create_account("bob", Some("password1"), 0).unwrap();
    let (_, s) = api.call("POST", "/v1/auth/login", None, Some(json!({"username": "bob", "password": "password1"}))).await;
    let session = s["token"].as_str().unwrap().to_string();
    assert_eq!(s["password_reset"], json!(false));

    // no login code for a Discord account that isn't linked
    let (st, _) = api.call("POST", "/v1/discord/codes", Some(&api.bot), Some(json!({"discord_id": "77", "purpose": "login"}))).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, c) = api.call("POST", "/v1/discord/codes", Some(&api.bot), Some(json!({"discord_id": "77", "purpose": "link"}))).await;
    assert_eq!(st, StatusCode::CREATED);
    let code = c["code"].as_str().unwrap().to_string();
    assert!(c["expires_at"].as_i64().unwrap() > lmoxide_db::now());

    let (st, e) = api.call("POST", "/v1/me/discord", Some(&session), Some(json!({"code": "AAAA-AAAA"}))).await;
    assert_eq!((st, &e["error"]["code"]), (StatusCode::UNAUTHORIZED, &json!("bad_code")));
    let (st, a) = api.call("POST", "/v1/me/discord", Some(&session), Some(json!({"code": code.to_lowercase()}))).await;
    assert_eq!((st, &a["discord_id"]), (StatusCode::OK, &json!("77")));
    // used up, and the account is linked now
    assert_eq!(api.call("POST", "/v1/me/discord", Some(&session), Some(json!({"code": code}))).await.0, StatusCode::CONFLICT);
    let (st, _) = api.call("POST", "/v1/discord/codes", Some(&api.bot), Some(json!({"discord_id": "77", "purpose": "link"}))).await;
    assert_eq!(st, StatusCode::CONFLICT);

    // a login code: a password_reset session that sets a password without the old one
    let (_, c) = api.call("POST", "/v1/discord/codes", Some(&api.bot), Some(json!({"discord_id": "77", "purpose": "login"}))).await;
    let code = c["code"].as_str().unwrap().replace('-', "");
    let (st, s) = api.call("POST", "/v1/auth/code", None, Some(json!({"code": code}))).await;
    assert_eq!((st, &s["password_reset"], &s["account"]["username"]), (StatusCode::OK, &json!(true), &json!("bob")));
    let reset = s["token"].as_str().unwrap().to_string();
    assert_eq!(api.call("POST", "/v1/auth/code", None, Some(json!({"code": code}))).await.0, StatusCode::UNAUTHORIZED, "once");

    // an ordinary session still needs the current password
    let (st, _) = api.call("POST", "/v1/me/password", Some(&session), Some(json!({"new_password": "password2"}))).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    let (st, _) = api.call("POST", "/v1/me/password", Some(&reset), Some(json!({"new_password": "password2"}))).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _) = api.call("POST", "/v1/auth/login", None, Some(json!({"username": "bob", "password": "password2"}))).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(api.call("GET", "/v1/me", Some(&reset), None).await.0, StatusCode::UNAUTHORIZED, "a password change ends sessions");

    // wrong codes count towards the login lockout
    for _ in 0..10 {
        api.call("POST", "/v1/auth/code", None, Some(json!({"code": "ZZZZ-ZZZZ"}))).await;
    }
    assert_eq!(api.call("POST", "/v1/auth/code", None, Some(json!({"code": "ZZZZ-ZZZZ"}))).await.0, StatusCode::TOO_MANY_REQUESTS);
}
