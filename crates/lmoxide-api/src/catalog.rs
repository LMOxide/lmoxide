//! The game data catalogs under `/v1/catalog` (scope read): effect packages, monster data looks,
//! animations, character states, and the sounds of effect packages. They are generated from the
//! client by `tools/re/extract_*.py` into `data/` and compiled in; each is serialized once and
//! sent with an `ETag`, so a client that has it gets `304 Not Modified`.

use std::{
    collections::{HashMap, HashSet},
    path::{Path as FsPath, PathBuf},
    sync::{Arc, LazyLock},
};

use axum::{
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use lmoxide_db::Scope;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    AppState,
    auth::Auth,
    error::{ApiError, ApiResult},
};

pub fn router() -> OpenApiRouter<Arc<AppState>> {
    OpenApiRouter::new()
        .routes(routes!(effect_catalog))
        .routes(routes!(effect_sound))
        .routes(routes!(monster_catalog))
        .routes(routes!(animation_catalog))
        .routes(routes!(bcc_states))
}

/// A catalog's JSON, serialized once, and its entity tag.
pub struct Cached {
    body: Vec<u8>,
    etag: HeaderValue,
}

impl Cached {
    fn new<T: Serialize>(value: &T) -> Cached {
        let body = serde_json::to_vec(value).expect("catalog JSON");
        // FNV-1a over the body: changes whenever the data does (a new server build)
        let hash = body.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
        Cached { body, etag: HeaderValue::from_str(&format!("\"{hash:016x}\"")).expect("etag") }
    }

    /// The JSON, or 304 if the request's `If-None-Match` has this version.
    fn respond(&self, headers: &HeaderMap) -> Response {
        let cache = [
            (header::CACHE_CONTROL, HeaderValue::from_static("private, max-age=3600")),
            (header::ETAG, self.etag.clone()),
        ];
        let fresh = headers
            .get(header::IF_NONE_MATCH)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.split(',').any(|t| t.trim() == self.etag.to_str().unwrap_or("") || t.trim() == "*"));
        if fresh {
            return (StatusCode::NOT_MODIFIED, cache).into_response();
        }
        (cache, [(header::CONTENT_TYPE, HeaderValue::from_static("application/json"))], self.body.clone()).into_response()
    }
}

fn parse<T: DeserializeOwned>(name: &str, json: &str) -> T {
    serde_json::from_str(json).unwrap_or_else(|e| panic!("data/{name}: {e}"))
}

// ---- effect packages ----

/// The client's effect packages and particle systems (data/effect_catalog.json,
/// tools/re/extract_effect_catalog.py).
#[derive(Serialize, Deserialize, ToSchema)]
pub struct EffectCatalog {
    pub effects: Vec<Effect>,
}

#[derive(Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Effect {
    /// What EffectPackageIIR plays (`!fx <id>`): timelines of particles, sounds, animation
    /// attributes, Lua, ... Packages have no names of their own: `used_by` names them.
    Effectpackage(EffectPackage),
    /// A particle effect packages refer to by resource name.
    Particlesystem(ParticleSystem),
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct EffectPackage {
    /// RDB 1000622 id.
    pub id: u32,
    pub timelines: Vec<Timeline>,
    pub particles: Vec<Particle>,
    pub sounds: Vec<Sound>,
    /// Animation attributes it sets (`limbostart`, `chainattack`, ...; GET /v1/catalog/animations).
    pub animation_attributes: Vec<String>,
    /// Code hooks its timelines start at (`onhit`, `charge`, `execute`, ...): `!fx <id> <hook>`.
    pub code_hooks: Vec<String>,
    /// Lua snippets (at most 400 characters each).
    pub lua: Vec<String>,
    pub looping: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub used_by: Vec<UsedBy>,
}

/// One part of a package: a particle, a sound, an animation attribute, Lua, ...
#[derive(Serialize, Deserialize, ToSchema)]
pub struct Timeline {
    /// particle, sound, emotionfxattribute, lua, environmenteffect, movement, camerapath, ...
    #[serde(rename = "type")]
    pub kind: String,
    pub name: String,
    /// Seconds after the package (or its code hook) starts.
    pub start: f32,
    /// Seconds; `null` when the part sets none (it lasts as long as its content, e.g. a sound)
    /// or lasts until the package is stopped (`until_stopped`). 0: an instant (Lua, an attribute).
    pub duration: Option<f32>,
    /// The designers' "forever": it lasts until the package is stopped.
    pub until_stopped: bool,
    /// self, target, client, ground, other, mount.
    pub target: String,
    /// The code hook that starts this part's timeline (`null`: when the package starts).
    pub hook: Option<String>,
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct Particle {
    /// RDB 1010207 id (`null`: not in the client's name index).
    pub id: Option<u32>,
    /// Resource name (`null`: a package names it by id and the index has no name for it).
    pub name: Option<String>,
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct Sound {
    /// As the package names it, e.g. `sfx\swishes\swish_blade_throw_1.wav`.
    pub path: String,
    /// RDB 1020002 id (Ogg Vorbis): GET /v1/catalog/sounds/{id}. `null`: not in the client.
    pub id: Option<u32>,
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct ParticleSystem {
    pub id: u32,
    pub emitters: Vec<String>,
    pub scripts: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub used_by: Vec<UsedBy>,
}

/// An item template (RDB 1000620) or spell (1000621) whose data mention the id (a byte search:
/// a few may be coincidences).
#[derive(Serialize, Deserialize, ToSchema)]
pub struct UsedBy {
    /// `item` or `spell`
    pub rdb: String,
    pub id: u32,
    pub name: Option<String>,
}

static EFFECTS: LazyLock<(EffectCatalog, Cached)> = LazyLock::new(|| {
    let c: EffectCatalog = parse("effect_catalog.json", include_str!("../../../data/effect_catalog.json"));
    let cached = Cached::new(&c);
    (c, cached)
});

/// The client's effect packages and particle systems: timelines, particles, sounds, animation
/// attributes, code hooks, Lua, and the items and spells that use them
/// (tools/re/extract_effect_catalog.py). Scope: read.
#[utoipa::path(get, path = "/v1/catalog/effects", tag = "catalog", responses((status = OK, body = EffectCatalog), (status = NOT_MODIFIED)))]
async fn effect_catalog(auth: Auth, headers: HeaderMap) -> ApiResult<Response> {
    auth.require(Scope::Read)?;
    Ok(EFFECTS.1.respond(&headers))
}

/// The sound ids effect packages play: only these are served.
static SOUND_IDS: LazyLock<HashSet<u32>> = LazyLock::new(|| {
    EFFECTS
        .0
        .effects
        .iter()
        .filter_map(|e| match e {
            Effect::Effectpackage(p) => Some(p.sounds.iter().filter_map(|s| s.id)),
            Effect::Particlesystem(_) => None,
        })
        .flatten()
        .collect()
});

/// A sound an effect package plays (`sounds[].id`), as Ogg Vorbis from the client's RDB (type
/// 1020002). Needs `--client-dir`; 503 without it. Scope: read.
#[utoipa::path(get, path = "/v1/catalog/sounds/{id}", tag = "catalog", params(("id" = u32, Path)),
    responses((status = OK, content_type = "audio/ogg", body = Vec<u8>), (status = NOT_FOUND), (status = SERVICE_UNAVAILABLE)))]
async fn effect_sound(State(state): State<Arc<AppState>>, auth: Auth, Path(id): Path<u32>) -> ApiResult<Response> {
    auth.require(Scope::Read)?;
    if !SOUND_IDS.contains(&id) {
        return Err(ApiError::not_found("sound"));
    }
    let Some(dir) = state.cfg.client_dir.clone() else {
        return Err(ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "no_client", "the API runs without --client-dir"));
    };
    let body = tokio::task::spawn_blocking(move || rdb_body(&dir, SOUNDS, id))
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::not_found("sound"))?;
    Ok(([(header::CONTENT_TYPE, "audio/ogg"), (header::CACHE_CONTROL, "private, max-age=86400")], Body::from(body)).into_response())
}

/// RDB type of the sounds (Ogg Vorbis).
const SOUNDS: u32 = 1020002;

/// A resource body from a client's RDB: `RDB/RDBHashIndex.bin` ("RDHI", u32 version, u32
/// resource count, u32 type count, then per type u32 type, u32 n and n records of 47 bytes: u32
/// id, u32 size, u32 ?, md5[16] naming the file `RDB/<type>/<MD5>.rdbres`, ...). A file starts
/// with (u32 type, u32 id, u32 ?) unless it is the bare body (tools/re/rdb.py).
fn rdb_body(client: &FsPath, rtype: u32, id: u32) -> Option<Vec<u8>> {
    static INDEX: std::sync::OnceLock<Option<HashMap<(u32, u32), String>>> = std::sync::OnceLock::new();
    let index = INDEX.get_or_init(|| read_index(&client.join("RDB/RDBHashIndex.bin"))).as_ref()?;
    let hash = index.get(&(rtype, id))?;
    let file: PathBuf = client.join(format!("RDB/{rtype}/{hash}.rdbres"));
    let b = std::fs::read(file).ok()?;
    let wrapped = b.len() >= 12 && u32::from_le_bytes(b[0..4].try_into().ok()?) == rtype;
    Some(if wrapped { b[12..].to_vec() } else { b })
}

fn read_index(path: &FsPath) -> Option<HashMap<(u32, u32), String>> {
    let d = std::fs::read(path).ok()?;
    let u32_at = |o: usize| d.get(o..o + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
    if d.get(0..4)? != b"RDHI" {
        return None;
    }
    let types = u32_at(12)?;
    let mut out = HashMap::new();
    let mut o = 16;
    for _ in 0..types {
        let (t, n) = (u32_at(o)?, u32_at(o + 4)?);
        o += 8;
        for _ in 0..n {
            let id = u32_at(o)?;
            let md5 = d.get(o + 12..o + 28)?;
            out.insert((t, id), md5.iter().map(|b| format!("{b:02X}")).collect());
            o += 47;
        }
    }
    Some(out)
}

// ---- monster data ----

/// Every monster data (RDB 1010514): the looks `!skin <id>` takes (data/monster_data.json,
/// tools/re/extract_monster_data.py).
#[derive(Serialize, Deserialize, ToSchema)]
pub struct MonsterCatalog {
    pub monster_data: Vec<MonsterData>,
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct MonsterData {
    pub id: u32,
    /// The designers' name, e.g. `minotaur_monster_boss`.
    pub name: String,
    /// Its motions (GET /v1/catalog/animations `motion_sets`).
    pub motion_set: Option<u32>,
    /// Its anim graph (`anim_graphs`): which animation attributes it reacts to.
    pub anim_graph: Option<u32>,
    pub actor: Option<u32>,
    pub mesh: Option<u32>,
    /// The effect package played when it is defeated.
    pub death_effect_package: Option<u32>,
    /// What the world places with this look.
    pub used_by: Vec<MonsterUser>,
    /// Minifigure item templates with this body (GET /v1/minifigures).
    pub minifigures: Vec<u32>,
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct MonsterUser {
    pub name: String,
    /// enemy, smashable or npc
    pub kind: String,
}

static MONSTERS: LazyLock<Cached> =
    LazyLock::new(|| Cached::new(&parse::<MonsterCatalog>("monster_data.json", include_str!("../../../data/monster_data.json"))));

/// Every monster data (look) in the client, with its motion set, anim graph, death effect and
/// what uses it: the choices for `!skin <id>`. Scope: read.
#[utoipa::path(get, path = "/v1/catalog/monsters", tag = "catalog", responses((status = OK, body = MonsterCatalog), (status = NOT_MODIFIED)))]
async fn monster_catalog(auth: Auth, headers: HeaderMap) -> ApiResult<Response> {
    auth.require(Scope::Read)?;
    Ok(MONSTERS.respond(&headers))
}

// ---- animations ----

/// Motion sets, anim graphs and the animation attributes effect packages set
/// (data/animations.json, tools/re/extract_animations.py).
#[derive(Serialize, Deserialize, ToSchema)]
pub struct AnimationCatalog {
    pub attributes: Vec<AnimationAttribute>,
    pub motion_sets: Vec<MotionSet>,
    pub anim_graphs: Vec<AnimGraph>,
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct AnimationAttribute {
    /// e.g. `limbostart`
    pub name: String,
    /// Anim graphs with this parameter.
    pub graphs: Vec<u32>,
    /// Effect packages that set it.
    pub packages: Vec<u32>,
    /// The package `!anim <name>` plays.
    pub package: Option<u32>,
}

/// RDB 1050004: a look's motions by id.
#[derive(Serialize, Deserialize, ToSchema)]
pub struct MotionSet {
    pub id: u32,
    pub name: String,
    pub motions: Vec<Motion>,
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct Motion {
    /// e.g. `idle`, `ability_1`
    pub id: String,
    /// The exported .xsm file.
    pub file: String,
}

/// RDB 1050003: an EMotion FX anim graph (blend setup).
#[derive(Serialize, Deserialize, ToSchema)]
pub struct AnimGraph {
    pub id: u32,
    pub name: String,
    /// Its parameters: the animation attributes it reacts to.
    pub attributes: Vec<String>,
    /// The motion ids its nodes play.
    pub motions: Vec<String>,
}

static ANIMATIONS: LazyLock<Cached> =
    LazyLock::new(|| Cached::new(&parse::<AnimationCatalog>("animations.json", include_str!("../../../data/animations.json"))));

/// The client's animations: animation attributes (what `!anim <attribute>` plays), motion sets
/// and anim graphs. Scope: read.
#[utoipa::path(get, path = "/v1/catalog/animations", tag = "catalog", responses((status = OK, body = AnimationCatalog), (status = NOT_MODIFIED)))]
async fn animation_catalog(auth: Auth, headers: HeaderMap) -> ApiResult<Response> {
    auth.require(Scope::Read)?;
    Ok(ANIMATIONS.respond(&headers))
}

// ---- character states ----

/// The character states the client defines (stat 20, BCCState: `!state <value>`) and substates,
/// from its behaviour tree (data/bcc_states.json, tools/re/extract_bcc_states.py).
#[derive(Serialize, Deserialize, ToSchema)]
pub struct BccStates {
    pub states: Vec<BccState>,
    pub substates: Vec<BccState>,
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct BccState {
    pub value: u32,
    pub name: String,
    /// The action groups allowed in it.
    pub groups: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<u32>,
    /// What the client does in it, where known.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub about: String,
}

static BCC_STATES: LazyLock<Cached> = LazyLock::new(|| Cached::new(&parse::<BccStates>("bcc_states.json", include_str!("../../../data/bcc_states.json"))));

/// The character states (`!state <value>`) and substates the client knows, by name. Scope: read.
#[utoipa::path(get, path = "/v1/catalog/bcc-states", tag = "catalog", responses((status = OK, body = BccStates), (status = NOT_MODIFIED)))]
async fn bcc_states(auth: Auth, headers: HeaderMap) -> ApiResult<Response> {
    auth.require(Scope::Read)?;
    Ok(BCC_STATES.respond(&headers))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalogs_parse() {
        let effects = &EFFECTS.0.effects;
        assert!(effects.len() > 2500);
        assert!(effects.iter().any(|e| matches!(e, Effect::Effectpackage(p) if !p.code_hooks.is_empty())));
        assert!(!MONSTERS.body.is_empty() && !ANIMATIONS.body.is_empty() && !BCC_STATES.body.is_empty());
    }

    #[test]
    fn etag_answers_304() {
        let mut h = HeaderMap::new();
        assert_eq!(BCC_STATES.respond(&h).status(), StatusCode::OK);
        h.insert(header::IF_NONE_MATCH, BCC_STATES.etag.clone());
        assert_eq!(BCC_STATES.respond(&h).status(), StatusCode::NOT_MODIFIED);
    }
}
