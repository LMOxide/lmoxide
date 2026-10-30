//! Renaming through the whole router: an account's own username (with its cooldown) and an
//! admin renaming characters.

use std::{net::SocketAddr, sync::Arc};

use axum::{
    Router,
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use clap::Parser;
use http_body_util::BodyExt;
use lmoxide_db::{Character, Db};
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
}

impl Api {
    fn new() -> Self {
        let cfg = Args::parse_from(["api"]).cfg;
        let db = Arc::new(Db::open_in_memory().unwrap());
        Api { app: crate::app(cfg, db.clone()).unwrap(), db }
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

    /// An account with a password and a login session.
    fn account(&self, username: &str, gm_level: u32) -> (u32, String) {
        let a = self.db.create_account(username, Some("password1"), gm_level).unwrap();
        (a.id, self.db.create_session(a.id, 3600).unwrap())
    }

    fn character(&self, account: u32, name: &str) {
        let c = Character {
            name: name.into(),
            mode: "maxed".into(),
            playfield: 1200,
            pos: [0.0; 3],
            team: Vec::new(),
            stars: 0,
            upgrades: Default::default(),
            tokens: Default::default(),
            xp: 0,
            level: 1,
            minifigure_levels: Default::default(),
            purchased_upgrades: Default::default(),
            tags: Default::default(),
            counters: Default::default(),
            defeats: Default::default(),
            visits: Default::default(),
            finds: Default::default(),
            smashed: Default::default(),
            collection: None,
            quests: Default::default(),
            story: Default::default(),
            speed: 1.0,
        };
        self.db.save_character(account, account, &c).unwrap();
    }
}

#[tokio::test]
async fn own_username_with_cooldown() {
    let api = Api::new();
    let (_, player) = api.account("player", 0);
    let (_, admin) = api.account("admin", 4);
    api.account("taken", 0);
    let (_, config) = api.call("GET", "/v1/config", None, None).await;
    assert_eq!(config["username_cooldown_days"], 7);

    let change = |name: &str, password: Option<&str>| json!({"new_username": name, "current_password": password});
    let (s, _) = api.call("POST", "/v1/me/username", Some(&player), Some(change("player2", None))).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, _) = api.call("POST", "/v1/me/username", Some(&player), Some(change("player2", Some("wrong-password")))).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, _) = api.call("POST", "/v1/me/username", Some(&player), Some(change("TAKEN", Some("password1")))).await;
    assert_eq!(s, StatusCode::CONFLICT);
    let (s, _) = api.call("POST", "/v1/me/username", Some(&player), Some(change("no spaces", Some("password1")))).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    let (s, me) = api.call("POST", "/v1/me/username", Some(&player), Some(change("player2", Some("password1")))).await;
    assert_eq!((s, me["username"].as_str()), (StatusCode::OK, Some("player2")));
    assert!(me["username_changed_at"].is_i64());
    // the session stays; the next change waits
    let (s, e) = api.call("POST", "/v1/me/username", Some(&player), Some(change("player3", Some("password1")))).await;
    assert_eq!((s, e["error"]["code"].as_str()), (StatusCode::TOO_MANY_REQUESTS, Some("username_cooldown")));
    assert!(api.db.account_by_name("player2").unwrap().is_some());

    // admins don't wait
    for name in ["admin2", "admin3"] {
        let (s, _) = api.call("POST", "/v1/me/username", Some(&admin), Some(change(name, Some("password1")))).await;
        assert_eq!(s, StatusCode::OK);
    }
    // an admin renaming someone else doesn't start their cooldown
    let other = api.db.account_by_name("taken").unwrap().unwrap();
    let (s, a) = api.call("PATCH", &format!("/v1/accounts/{}", other.id), Some(&admin), Some(json!({"username": "taken2"}))).await;
    assert_eq!((s, a["username_changed_at"].clone()), (StatusCode::OK, Value::Null));
}

#[tokio::test]
async fn admins_rename_characters() {
    let api = Api::new();
    let (player_id, player) = api.account("player", 0);
    let (admin_id, admin) = api.account("admin", 4);
    api.character(player_id, "Fantastic Sparky Burrito");
    api.character(admin_id, "Grumpy Maximum Axe");

    let rename = |name: &str| json!({"name": name});
    let path = format!("/v1/characters/{player_id}");
    let (s, _) = api.call("PATCH", &path, Some(&player), Some(rename("New Name"))).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = api.call("PATCH", &path, Some(&admin), Some(rename("One Two Three Four"))).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    let (s, _) = api.call("PATCH", &path, Some(&admin), Some(rename("grumpy MAXIMUM axe"))).await;
    assert_eq!(s, StatusCode::CONFLICT);
    let (s, _) = api.call("PATCH", "/v1/characters/12345", Some(&admin), Some(rename("New Name"))).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    // checked, then renamed by the game server (none set up here)
    let (s, _) = api.call("PATCH", &path, Some(&admin), Some(rename("New Name"))).await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE);
}
