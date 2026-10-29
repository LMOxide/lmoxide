//! Game server: the playfield connection the client opens after `LoginConnectReady`
//! (`ClientToGameserverRPC` / `GameserverToClientRPC`, then the gamecode endpoints).

use crate::instance::Pf;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use bytes::Bytes;

use lmoxide_net::{
    Endpoint, Reader, Writer, iir,
    methods::{
        client_to_gameserver_rpc as agent, cs_player_agent, game_char_agent, game_char_agent_project, game_char_interface,
        game_char_interface_project,
        gameserver_to_client_rpc as client,
    },
    proto::{
        PlayfieldProxyData, ProtoAcgItem, ProtoCoreCharacterIir, ProtoFullCharacterIir, ProtoInventoryGameEntry,
        ProtoGameCodeDynelIir, ProtoId32, ProtoQuat,
        ProtoVec3, proto_core_character_iir,
    },
    types::Id32,
};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::{
    Config,
    collection::Collection,
    conn::Conn,
    state,
    store::{Character, Store},
    world,
};

pub async fn handle(mut conn: Conn, cfg: Arc<Config>) -> anyhow::Result<()> {
    conn.local = Endpoint::ClientToGameserverRPC;
    let status: i32 = std::env::var("LMO_GAME_AUTH_STATUS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    let store = Store;
    let mut character = 0;
    let mut me = new_character(String::new(), cfg.start_playfield);
    // the running copy of the character's playfield it is in (`me.playfield` is the playfield
    // saved: the copy is chosen anew on arrival, instance.rs)
    let mut place = Pf::main(me.playfield);
    let mut collection = Collection::new(&me.team, me.collection.as_ref());
    let pos = Arc::new(Mutex::new(START_POS));
    let mut last_save = std::time::Instant::now();
    let mut portals = Portals::default();
    // between movement reports: a player walking a path reaches portals the reports don't show
    let mut portal_tick = tokio::time::interval(PORTAL_TICK);
    portal_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // direction of the player's last movement (aims projectiles cast without a target)
    let mut heading = [0.0f32, 1.0];
    // the same, shared with tasks that fire later (held charges)
    let facing = Arc::new(Mutex::new(heading));
    // held charge abilities by command client id, fired on release
    let mut charges: HashMap<u32, Charge> = HashMap::new();
    let mut last_summon: Option<(u32, std::time::Instant)> = None;
    // the last charge fired automatically, and when the client last re-sent it since (see
    // CHARGE_RECAST_GAP)
    let auto_fired: Arc<Mutex<Option<(u32, std::time::Instant)>>> = Arc::new(Mutex::new(None));
    let (outbox, mut inbox) = mpsc::unbounded_channel::<world::Out>();
    let mut presence: Option<world::Presence> = None;
    // online for friends and teams while the character is in play
    let mut community: Option<crate::community::Guard> = None;
    // the account's effective GM level and its entry in the online list (control interface)
    let mut gm_level = 0;
    let mut account_id = 0;
    let mut _online: Option<crate::sessions::OnlineGuard> = None;
    let mut tour_started = false;
    // the others' copies of this player's casts, by client command id (ended on release/abort)
    let mut observed_casts: HashMap<u32, (u32, Id32, u32, std::time::Instant)> = HashMap::new();
    // the tutorial, while in it
    let mut tutorial: Option<crate::tutorial::Run> = None;
    // clearing the playfield in this visit: its own enemies, and those defeated so far
    let mut residents: std::collections::HashSet<Id32> = Default::default();
    let mut defeated_here: std::collections::HashSet<Id32> = Default::default();
    // the playfield was cleared in this visit (by this player or a teammate here)
    let mut cleared_here = false;
    // a test setup changed the team (sent again at the next CharInPlay), and the item id salt
    let mut team_changed = false;
    // in play before: a later CharInPlayIIR_t is the arrival after a teleport
    let mut was_in_play = false;
    let mut team_salt = 0u64;
    // the active minifigure was smashed: the character is in Limbo until the next one takes over
    let mut in_limbo = false;
    // `!fx`: the package playing (package, instance) and its place in the list
    let mut playing_fx: Option<(u32, u32)> = None;
    let mut playing_fx_at: Option<usize> = None;
    // the area quests in the client's tracker (quests.rs)
    let mut quest_tracker = crate::quests::Tracker::default();
    loop {
        // a control interface command being handled: dropped (signalling "done") when this
        // iteration ends, so its caller can collect the chat answers (chat::capture)
        let mut _command_done = None;
        let rpc = tokio::select! {
            rpc = conn.recv() => match rpc? {
                Some(rpc) => rpc,
                None => break,
            },
            _ = portal_tick.tick() => {
                if presence.is_some() {
                    let id = Id32::new(crate::territory::CHARACTER_TYPE, character);
                    let here = world::live_position(id).unwrap_or_else(|| *pos.lock().unwrap());
                    portals.check(&mut conn, character, place, here).await?;
                    // quest areas also after teleports and throws (no movement report)
                    let playfield = place;
                    let records = crate::quests::moved(&mut quest_tracker, &mut me, character, playfield, here);
                    send_records(&mut conn, character, records).await?;
                }
                continue;
            }
            Some(out) = inbox.recv() => {
                if let world::Out::Command(text, done) = out {
                    _command_done = done;
                    // handled like the same line typed into the local chat
                    let args = Writer::new().string(&text).i32(0).finish();
                    let agent = Endpoint::GameCharAgent.proto(None);
                    lmoxide_net::rpc::Rpc::new(agent.clone(), agent, game_char_agent::VICINITY_CHAT_MESSAGE, args)
                } else {
                    // a teammate's progress counts as the player's own, minus its consequences
                    let (out, shared) = match out {
                        world::Out::TeamProgress(event) => (world::Out::Progress(event), true),
                        out => (out, false),
                    };
                    match out {
                        world::Out::Records(records) => {
                            let blob = iir::pass_blob(&records);
                            conn.send_as(Some(character), Endpoint::GameserverToClientRPC, client::PASS_BLOB, blob).await?;
                        }
                        world::Out::Rpc(endpoint, method, args) => conn.send_as(Some(character), endpoint, method, args).await?,
                        world::Out::DevTeleport(playfield, at, label) => {
                            info!(character, playfield, ?at, %label, "dev tour");
                            crate::chat::system(character, &label);
                            let at = [at[0] + 6.0, at[1], at[2]];
                            teleport(&mut conn, &store, character, &mut me, &collection, &mut presence, &pos, crate::instance::towards(place, playfield), &mut place, None, Some((at, -std::f32::consts::FRAC_PI_2)))
                                .await?;
                        }
                        world::Out::Progress(event) => {
                            if let crate::achievements::Event::Cleared { playfield } = &event {
                                // once per visit: both the player's own count and a teammate's
                                // clear arrive
                                if *playfield != me.playfield || cleared_here {
                                    continue;
                                }
                                cleared_here = true;
                            }
                            if let crate::achievements::Event::Defeated { id, smashable: false, playfield, .. } = &event
                                && *playfield == me.playfield
                                && residents.contains(id)
                                && defeated_here.insert(*id)
                                && defeated_here.len() == residents.len()
                            {
                                // the last of the playfield's enemies: cleared, also for the teammates
                                // here (one who came in later never saw every defeat)
                                let cleared = crate::achievements::Event::Cleared { playfield: *playfield };
                                crate::community::share(character, place, &cleared);
                                let _ = outbox.send(world::Out::Progress(cleared));
                            }
                            if let crate::achievements::Event::Built { name } = &event
                                && !shared
                                && name == crate::cannon::BUILDIE
                                && me.playfield == crate::cannon::PLAYFIELD
                            {
                                crate::cannon::fire(Id32::new(crate::territory::CHARACTER_TYPE, character), me.name.clone());
                            }
                            if let crate::achievements::Event::Defeated { name, playfield, .. } = &event
                                && name == crate::tutorial::BOSS
                                && *playfield == crate::tutorial::PLAYFIELD
                                && let Some(run) = tutorial.as_mut()
                            {
                                // the tutorial's boss: the music stops, the player watches Dreadleg
                                // escape and hears the last dialogue (recorded)
                                let stop = crate::tutorial::music_record(Id32::new(crate::territory::CHARACTER_TYPE, character), "CombatMusic_Stop");
                                conn.send_as(Some(character), Endpoint::GameserverToClientRPC, client::PASS_BLOB, iir::pass_blob(&[stop])).await?;
                                run.boss_defeated();
                                let _ = outbox.send(world::Out::Teleport(None, crate::tutorial::AFTER_BOSS, 0.0));
                            }
                            if let crate::achievements::Event::Defeated { smashable: false, level, boss, champion, .. } = &event
                                && crate::progression::enabled()
                            {
                                let records = gain_xp(&mut me, character, crate::progression::kill_xp(*level, *boss, *champion));
                                conn.send_as(Some(character), Endpoint::GameserverToClientRPC, client::PASS_BLOB, iir::pass_blob(&records)).await?;
                            }
                            let out = progress(&mut me, &collection, character, Some((Some(&event), 1)));
                            send_progress(&mut conn, character, place, &mut me, &mut collection, out).await?;
                            let records = crate::quests::on_event(&mut quest_tracker, &mut me, character, &event, 1);
                            send_records(&mut conn, character, records).await?;
                        }
                        world::Out::Kick(reason) => {
                            info!(character, ?reason, "kicked");
                            if let Some(reason) = reason {
                                crate::chat::system(character, &reason);
                                // let the chat line arrive before the connection drops
                                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                            }
                            break;
                        }
                        world::Out::Command(..) | world::Out::TeamProgress(_) => unreachable!(),
                    world::Out::Window(window) => portals.open_window(&mut conn, character, window).await?,
                    world::Out::RemoveAchievements(which, reply) => {
                        let (gone, rpcs) = crate::achievements::remove(&mut me, which.as_deref());
                        info!(character, removed = ?gone, all = which.is_none(), "achievements removed");
                        for (endpoint, method, args) in rpcs {
                            conn.send_as(Some(character), endpoint, method, args).await?;
                        }
                        if let Some(&(_, template)) = me.team.first() {
                            crate::auras::apply(Id32::new(crate::territory::CHARACTER_TYPE, character), template, &me.tags, false);
                        }
                        if let Err(e) = store.save(character, &me) {
                            warn!(character, "saving character failed: {e:#}");
                        }
                        let tx = reply.lock().unwrap().take();
                        match tx {
                            Some(tx) => {
                                let _ = tx.send(gone);
                            }
                            // (!unach)
                            None => crate::chat::system(character, &format!("{} achievements removed{}", gone.len(), if which.is_none() { ", and the journal and counters (they show again after logging in)" } else { "" })),
                        }
                    }
                    world::Out::Teleport(playfield, at, rot) => {
                        let playfield = playfield.unwrap_or(place.model());

                        teleport(&mut conn, &store, character, &mut me, &collection, &mut presence, &pos, crate::instance::towards(place, playfield), &mut place, None, Some((at, rot)))
                            .await?;
                    }
                    world::Out::Smashed(template) => {
                        // locked for the rebuild time; the client switches to the next minifigure
                        if let std::collections::btree_map::Entry::Vacant(e) = me.smashed.entry(template) {
                            let until = crate::smash::now_ms() + crate::smash::REBUILD.as_millis() as i64;
                            e.insert(until);
                            info!(character, template, "minifigure smashed");
                            let args = crate::smash::lock_args(template, until);
                            conn.send_as(Some(character), Endpoint::GameCharInterface, game_char_interface::LOCK_TEMPLATE, args).await?;
                            schedule_rebuild(&outbox, template, until);
                            // the client switches to the next team member itself (health 0): the first
                            // of slots 6, 8, 20 that isn't smashed (ItemHolder::
                            // TrySwitchMinifigureFromFatigued); the server follows. With the whole
                            // team smashed the character falls into Limbo (the death dialog).
                            let next = me
                                .team
                                .iter()
                                .enumerate()
                                .filter(|&(_, &(_, t))| t != template && !me.smashed.contains_key(&t))
                                .min_by_key(|&(_, &(slot, _))| slot)
                                .map(|(i, _)| i);
                            if let Some(i) = next.filter(|_| me.team.first().is_some_and(|&(_, t)| t == template)) {
                                me.team[..=i].rotate_right(1);
                                let id = Id32::new(crate::territory::CHARACTER_TYPE, character);
                                let new_active = me.team[0].1;
                                let records = active_minifigure_records(id, new_active, me.upgrade(new_active), me.level(), &presence);
                                crate::auras::apply(id, new_active, &me.tags, true);
                                conn.send_as(Some(character), Endpoint::GameserverToClientRPC, client::PASS_BLOB, iir::pass_blob(&records)).await?;
                            }
                            if me.team.iter().all(|(_, t)| me.smashed.contains_key(t)) {
                                let id = Id32::new(crate::territory::CHARACTER_TYPE, character);
                                in_limbo = true;
                                world::send_playfield(id, true, world::Out::Records(vec![stat_record(id, STAT_BCC_STATE, BCC_STATE_LIMBO)]));
                            }
                        }
                    }
                    world::Out::Rebuilt(template) => {
                        if me.smashed.get(&template).is_some_and(|&until| until <= crate::smash::now_ms()) {
                            me.smashed.remove(&template);
                            rebuild(&mut conn, character, &me, template).await?;
                        }
                    }
                    }
                    continue;
                }
            }
        };
        let mut r = Reader::new(&rpc.args);
        match (rpc.target(), rpc.method()) {
            (Some(Endpoint::ClientToGameserverRPC), agent::AUTHENTICATE2) => {
                if conn.authenticated {
                    anyhow::bail!("second Authenticate2");
                }
                let claimed = r.u32()?;
                let cookie = r.u32()?;
                let version = r.string()?;
                // `claimed` is the character: the login cookie's account must own it (an
                // account's first character has the account's id and may not be saved yet)
                let ip_key = format!("ip:{}", conn.peer.ip());
                // (a player the peer server sent here: their account here, by username)
                let arrived = crate::switch::arrival(cookie, claimed);
                let session = arrived.or_else(|| state::or_log(state::db().game_session(cookie), "session", None));
                let owner = arrived
                    .or_else(|| state::or_log(state::db().character_account(claimed), "character", None))
                    .unwrap_or(claimed);
                let account = (crate::security::allowed(&ip_key) && session.is_some() && session == Some(owner))
                    .then(|| state::or_log(state::db().account(owner), "account", None))
                    .flatten();
                let Some(account) = account.filter(|a| a.active_ban().is_none()) else {
                    warn!(character = claimed, %version, "Authenticate2 refused: invalid cookie or banned");
                    crate::security::failed(&ip_key);
                    let args = Writer::new().i32(0).finish();
                    conn.send_as(Some(claimed), Endpoint::GameserverToClientRPC, client::ACK_AUTHENTICATE, args).await?;
                    return Ok(());
                };
                character = claimed;
                conn.authenticated = true;
                crate::switch::set_away(account.id, false);
                // from the peer server, first visit: the character this server keeps under the
                // client's id
                if arrived.is_some() && store.load(character).is_none() {
                    let new = new_character(account.username.clone(), cfg.start_playfield);
                    match store.create(character, account.id, &new) {
                        Ok(()) => info!(account = account.id, character, "character made for a player from the peer server"),
                        Err(e) => warn!(character, "creating the character failed: {e:#}"),
                    }
                }
                // (chat finds the character by the account; one sent here by the peer server
                // didn't pick it in this server's territory)
                crate::sessions::set_character(account.id, character);
                account_id = account.id;
                gm_level = account.gm_level.max(cfg.default_gm_level);
                me = store.load(character).unwrap_or_else(|| Character {
                    name: crate::sessions::name(character).unwrap_or_else(|| account.username.clone()),
                    ..me.clone()
                });
                active_at_first_position(&mut me.team);
                // older saves had potions per session: they start with the new character's
                me.tokens.entry(TOKEN_HEALTH_POTIONS).or_insert((START_POTIONS, 0));
                // smashed minifigures stay smashed until their rebuild time, across logins
                let now = crate::smash::now_ms();
                me.smashed.retain(|_, until| *until > now);
                for (&template, &until) in &me.smashed {
                    crate::npc::set_smashed(Id32::new(crate::territory::CHARACTER_TYPE, character), template);
                    schedule_rebuild(&outbox, template, until);
                }
                _online = Some(crate::sessions::online(account.id, character, &account.username, &me.name, gm_level, outbox.clone()));
                *pos.lock().unwrap() = me.pos;
                collection = Collection::new(&me.team, me.collection.as_ref());
                // the copy LoginConnectReady named (territory.rs)
                place = crate::instance::arrival(me.playfield);
                info!(character, %version, name = %me.name, playfield = ?place, pos = ?me.pos, gm_level, "Authenticate2");
                let args = Writer::new().i32(status).finish();
                conn.send_as(Some(character), Endpoint::GameserverToClientRPC, client::ACK_AUTHENTICATE, args)
                    .await?;
                if !crate::sessions::territory_closed(account.id, std::time::Duration::from_secs(10)).await {
                    warn!(character, "territory connection still open; sending the world anyway");
                }
                // Give the client a moment to process the territory disconnect.
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                send_world(&mut conn, character, &me, place, &collection).await?;
                // achievements: the earned tags and counters again (live sent them after the full
                // update, before the client is in play, and not on playfield changes), and what
                // is due already
                let out = progress(&mut me, &collection, character, None);
                send_progress(&mut conn, character, place, &mut me, &mut collection, out).await?;
            }
            _ if !conn.authenticated => anyhow::bail!("{} before Authenticate2", rpc.describe()),
            (Some(Endpoint::ClientToGameserverRPC), agent::SIGNAL_PASS_BLOB) => {
                // The client's own IIRs (u32 length + records). Only headers are decoded; the
                // client sends one record per blob.
                let len = r.u32()? as usize;
                let blob = r.bytes(len, "blob")?;
                let mut br = Reader::new(blob);
                let h = iir::Header::read(&mut br)?;
                let rest = br.remaining();
                debug!(iir = h.name().unwrap_or("?"), identity = ?h.identity, rest = %crate::conn::hex(rest), "client IIR");
                match h.name() {
                    Some("CharInPlayIIR_t") => {
                        // Acknowledge: the client waits in WaitingToReceiveInPlay for this. Then the
                        // active team slot: a change of it makes the client apply the minifigure's
                        // item effects, including its abilities (shortcut slots 104/105); sent with
                        // the world it is too early and has no effect.
                        let mut records = vec![iir::record("CharInPlayIIR_t", h.identity, false, |w| w)];
                        records.push(crate::teleport::journal_record(h.identity, &crate::quests::completed(&me)));
                        let refresh = crate::collection::collection_refresh(character, &collection, &|t| me.item_state(t));
                        records.push(iir::record("UpdateInventoryIIR_t", h.identity, false, |w| w.proto(&refresh)));
                        // a test setup changed the team, or a teleport moved the active minifigure
                        // to the first position (active_at_first_position): the HUD shows the
                        // team as the client has it until this
                        if !was_in_play {
                            // after "relocate? Yes" the chat connection may have survived (it
                            // reconnects by itself), but the client dropped its channels
                            crate::chat::rejoin(character);
                        }
                        if std::mem::take(&mut team_changed) | std::mem::replace(&mut was_in_play, true) {
                            team_salt += 1 << 33;
                            let team = crate::collection::team_refresh(character, &me.team, &|t| me.item_state(t), team_salt);
                            records.push(iir::record("UpdateInventoryIIR_t", h.identity, false, |w| w.proto(&team)));
                        }
                        if let Some(&(slot, template)) = me.team.first() {
                            // ParentStatChanged only reacts to a different slot, and the client
                            // may already assume this one: clear it first.
                            records.push(int32_stat(h.identity, STAT_ACTIVE_WEAPON, 0));
                            records.push(int32_stat(h.identity, STAT_ACTIVE_WEAPON, slot));
                            records.extend(health_records(h.identity, template, me.upgrade(template), me.level()));
                        }
                        let echo = iir::pass_blob(&records);
                        // The HUD shows the active minifigure's left/right-click spells, but they
                        // can't be cast until learned: LearnSpell(int spell) for the whole team.
                        learn_team_spells(&mut conn, character, &me).await?;
                        // the potion: learned, and put in shortcut slot 100 (the HUD's potion button)
                        // with AddServerShortcut(int type = 2 spell, int position, uint32 template)
                        let args = Writer::new().i32(POTION_SPELL as i32).finish();
                        conn.send_as(Some(character), Endpoint::GameCharInterface, game_char_interface::LEARN_SPELL, args).await?;
                        let args = Writer::new().i32(2).i32(100).u32(POTION_SPELL).finish();
                        conn.send_as(Some(character), Endpoint::GameCharInterface, game_char_interface::ADD_SERVER_SHORTCUT, args)
                            .await?;
                        conn.send_as(Some(character), Endpoint::GameserverToClientRPC, client::PASS_BLOB, echo)
                            .await?;
                        // the World Map's teleport spots for this world, all unlocked
                        let args = crate::teleport::valid_spots_args(place, *pos.lock().unwrap());
                        conn.send_as(Some(character), Endpoint::GameCharInterface, game_char_interface::VALID_RESURRECTION_SPOTS, args)
                            .await?;
                        if community.is_none() {
                            community = Some(crate::community::login(character, &me.name, place, outbox.clone()));
                        }
                        // development: LMOXIDE_DEV_COMMANDS="!test 4;!sleep 10;!xp 10" runs test
                        // commands 5 s after the first entry into the world (as if typed into the
                        // chat); "!sleep <secs>" waits between them
                        if let Ok(commands) = std::env::var("LMOXIDE_DEV_COMMANDS")
                            && !tour_started
                        {
                            tour_started = true;
                            let outbox = outbox.clone();
                            tokio::spawn(async move {
                                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                                for command in commands.split(';').map(str::trim).filter(|c| !c.is_empty()) {
                                    // "!sleep <secs>" waits before the next one
                                    if let Some(secs) = command.strip_prefix("!sleep ").and_then(|t| t.trim().parse::<f32>().ok()) {
                                        tokio::time::sleep(std::time::Duration::from_secs_f32(secs)).await;
                                        continue;
                                    }
                                    let _ = outbox.send(world::Out::Command(command.to_string(), None));
                                }
                            });
                        }
                        // development: LMOXIDE_DEV_TOUR=<kind> teleports through a test command
                        // kind's targets (for screenshots), one every LMOXIDE_DEV_TOUR_SECS
                        if let Ok(kind) = std::env::var("LMOXIDE_DEV_TOUR")
                            && !tour_started
                        {
                            tour_started = true;
                            let secs = std::env::var("LMOXIDE_DEV_TOUR_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(20);
                            let outbox = outbox.clone();
                            tokio::spawn(async move {
                                let targets = crate::devcmd::tour_targets(&kind);
                                for (i, (label, playfield, at)) in targets.into_iter().enumerate() {
                                    tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
                                    let label = format!("tour {}: {label} in {playfield}", i + 1);
                                    if outbox.send(world::Out::DevTeleport(playfield, at, label)).is_err() {
                                        break;
                                    }
                                }
                            });
                        }
                        crate::community::entered_playfield(character, place);
                        if presence.is_none() {
                            let here = *pos.lock().unwrap();
                            let (id, snapshot, pos_handle) = (h.identity, me.clone(), pos.clone());
                            let pos = pos.clone();
                            let spawn = move || remote_spawn_records(id, &Character { pos: *pos.lock().unwrap(), ..snapshot.clone() }.spec(place));
                            presence = Some(world::join(id, place, pos_handle, spawn, outbox.clone()));
                            // the journal: a visit; clearing starts over
                            let _ = outbox.send(world::Out::Progress(crate::achievements::Event::Visited { playfield: me.playfield }));
                            residents = crate::npc::resident_enemies(place);
                            defeated_here.clear();
                            cleared_here = false;
                            report_finds(&me, here, &outbox);
                            // every arrival (a teleport within the playfield too) reloads the
                            // client's world: it forgot the quests
                            quest_tracker = crate::quests::Tracker::default();
                            let playfield = place;
                            let records = crate::quests::moved(&mut quest_tracker, &mut me, character, playfield, here);
                            send_records(&mut conn, character, records).await?;
                            // encounters: a side dungeon's champion, an area the player arrived in
                            crate::encounter::entered(place, here, &|n| me.defeats.contains_key(n));
                            crate::encounter::moved(id, place, here);
                            set_costume(id, monster_data_for(me.team.first().map_or(0, |&(_, t)| t)));
                            refresh_costumes(id);
                            if let Some(&(_, template)) = me.team.first() {
                                crate::auras::apply(id, template, &me.tags, true);
                            }
                            // the tutorial runs until its last dialogue gave its tag
                            // (a teleport within it, after the boss, keeps the run)
                            if me.playfield != crate::tutorial::PLAYFIELD || me.tags.contains_key(&crate::tutorial::DONE_TAG) {
                                tutorial = None;
                            } else if tutorial.is_none() {
                                tutorial = Some(crate::tutorial::Run::start(id, &me.name, here));
                            }
                            // the dialogue the live server started on arrival here (the jail's)
                            crate::knubot::arrived(id, &me.name, place, &|t| me.tags.contains_key(&t));
                            if let Some(record) = tutorial.as_mut().and_then(|t| t.moved(here)) {
                                conn.send_as(Some(character), Endpoint::GameserverToClientRPC, client::PASS_BLOB, iir::pass_blob(&[record])).await?;
                            }
                            if me.playfield == crate::tutorial::CREATION_PLAYFIELD {
                                // a new character: the client's creation script sends the name and
                                // minifigure set picked in the launcher (SetCharacterCreationData)
                                let args = Writer::new().bool(true).finish();
                                conn.send_as(Some(character), Endpoint::GameCharInterface, game_char_interface::SET_IS_FIRST_LOGIN, args).await?;
                                let record = crate::tutorial::creation_script_record(id);
                                conn.send_as(Some(character), Endpoint::GameserverToClientRPC, client::PASS_BLOB, iir::pass_blob(&[record])).await?;
                            }
                        }
                    }
                    Some(kind @ ("CharDCMoveIIR_t" | "MoveTaskIIR_c")) => {
                        if let Some(yaw) = (kind == "CharDCMoveIIR_t").then(|| reported_facing(rest)).flatten() {
                            heading = yaw;
                            *facing.lock().unwrap() = yaw;
                            world::set_facing(h.identity, yaw);
                        }
                        // the other players get the record as it is, as the original server echoed
                        // it: keyboard movement (CharDCMoveIIR_t) carries the velocity the other
                        // clients keep the character running with until the next update
                        // (SimpleChar_t::UpdatePlayerPos, Vehicle_t::SetVel). Turning it into a
                        // path per update (f1659fc) made them restart the run animation every
                        // 0.2 s and stop between updates.
                        if let Some(p) = reported_position(kind, rest) {
                            let before = *pos.lock().unwrap();
                            let (dx, dz) = (p[0] - before[0], p[2] - before[2]);
                            let moved = (dx * dx + dz * dz).sqrt();
                            if kind == "MoveTaskIIR_c" && moved > 0.2 {
                                heading = [dx / moved, dz / moved];
                                *facing.lock().unwrap() = heading;
                                world::set_facing(h.identity, heading);
                            }
                            *pos.lock().unwrap() = p;
                            // where they are between reports (what enemy shots hit)
                            if let Some(motion) = reported_motion(kind, rest) {
                                world::set_motion(h.identity, motion);
                            }
                            // jump pads (click-to-move reports the path's end and its start)
                            let from = (kind == "MoveTaskIIR_c").then(|| reported_position("CharDCMoveIIR_t", rest)).flatten();
                            crate::jumppad::moved(h.identity, place, &pos, p, from);
                            report_finds(&me, p, &outbox);
                            let playfield = place;
                            let records = crate::quests::moved(&mut quest_tracker, &mut me, character, playfield, p);
                            send_records(&mut conn, character, records).await?;
                            crate::encounter::moved(h.identity, place, p);
                            crate::plates::moved(place, from, p);
                            if let Some(record) = tutorial.as_mut().and_then(|t| t.moved(p)) {
                                conn.send_as(Some(character), Endpoint::GameserverToClientRPC, client::PASS_BLOB, iir::pass_blob(&[record])).await?;
                            }
                            // portals: where the player is now (a click-to-move report gives the path's end)
                            let here = world::live_position(h.identity).unwrap_or(p);
                            portals.check(&mut conn, character, place, here).await?;
                            if last_save.elapsed() > SAVE_INTERVAL {
                                last_save = std::time::Instant::now();
                                me.pos = p;
                                if let Err(e) = store.save(character, &me) {
                                    warn!(character, "saving character failed: {e:#}");
                                }
                            }
                        }
                        if let Some(p) = &presence {
                            world::broadcast(p.id(), vec![Bytes::copy_from_slice(blob)]);
                        }
                    }
                    Some("BehaviorControlCenterIIR_c") if rest.first() == Some(&BCC_FINISH_CHANNEL) => {
                        // Release of a held (channelled/charged) ability: same command, action 20.
                        // Fire the charged shot; the record is meant for the other players too.
                        if let Some(p) = &presence {
                            world::broadcast(p.id(), vec![Bytes::copy_from_slice(blob)]);
                        }
                        let client_id = parse_command(rest).and_then(|(m, _)| m.commandqueue_client_id);
                        end_observed(h.identity, client_id.and_then(|id| observed_casts.remove(&id)));
                        if let Some(c) = client_id.and_then(|id| charges.remove(&id)) {
                            fire_charge(&c, &me, h.identity, *pos.lock().unwrap(), heading);
                        } else {
                            crate::actions::release(h.identity, false);
                        }
                    }
                    Some("BehaviorControlCenterIIR_c") if rest.first() == Some(&BCC_CLIENT_ABORT) => {
                        // The client aborts a command (e.g. a jump during a charge) and waits for
                        // the server to confirm: same record, action 7 (abort one). A charge
                        // ends without firing.
                        let client_id = parse_command(rest).and_then(|(m, _)| m.commandqueue_client_id);
                        end_observed(h.identity, client_id.and_then(|id| observed_casts.remove(&id)));
                        if let Some(c) = client_id.and_then(|id| charges.remove(&id)) {
                            c.active.store(false, std::sync::atomic::Ordering::Relaxed);
                            info!(spell = c.spell, "charge aborted");
                        } else {
                            crate::actions::release(h.identity, false);
                        }
                        let mut reply = blob.to_vec();
                        reply[blob.len() - rest.len()] = BCC_ABORT_ONE;
                        conn.send_as(Some(character), Endpoint::GameserverToClientRPC, client::PASS_BLOB, iir::pass_blob(&[Bytes::from(reply)]))
                            .await?;
                    }
                    Some("BehaviorControlCenterIIR_c") if rest.first() == Some(&BCC_QUEUE_COMMAND) => {
                        // A queued command (minifigure swap, ability, ...). It completes only
                        // when the server answers with the same record as action 4 (success).
                        let command_type = rest.get(1..5).map_or(0, |b| u32::from_be_bytes(b.try_into().unwrap()));
                        let mut refused = false;
                        let mut stop_after_success = false;
                        let mut extra = Vec::new();
                        // the original server allowed an ability to execute (BCC action 10) only
                        // at its ItemDelay, with the hits
                        let mut can_execute_after: Option<std::time::Duration> = None;
                        // the cast, for the other players
                        let mut cast_spell = None;
                        let mut cast_target = None;
                        let mut cast_client_id = None;
                        match parse_command(rest) {
                            Some((msg, data)) => {
                                let target = msg.target.as_ref().map(|t| (t.r#type.unwrap_or(0), t.instance.unwrap_or(0)));
                                // MagicCommand data: u32, u32 spell, ...
                                let spell = (command_type == COMMAND_MAGIC)
                                    .then(|| data.get(4..8).map(|b| u32::from_be_bytes(b.try_into().unwrap())))
                                    .flatten();
                                cast_spell = spell;
                                cast_client_id = msg.commandqueue_client_id;
                                cast_target = target.map(|(kind, instance)| Id32::new(kind, instance)).filter(|t| t.instance != 0);
                                info!(command_type, ?spell, ?target, client_id = ?msg.commandqueue_client_id, data = %crate::conn::hex(data), "command");
                                let talk_to = target
                                    .map(|(kind, instance)| Id32::new(kind, instance))
                                    .and_then(|t| crate::npc::friendly(t).map(|(name, playfield)| (t, name, playfield)))
                                    .filter(|(_, name, playfield)| crate::knubot::has_dialogues(name, playfield.model()));
                                info!(?talk_to, "interaction target");
                                if command_type == crate::buildie::COMMAND_OBJECT_INTERACTION && let Some((npc, name, playfield)) = talk_to {
                                    // talking to an NPC with dialogues
                                    crate::knubot::talk(h.identity, &me.name, npc, &name, playfield);
                                } else if command_type == crate::buildie::COMMAND_OBJECT_INTERACTION
                                    && let Some((guid, action)) = interaction_object(data)
                                        .filter(|&(object, _)| object.kind == crate::scenery::SIMPLE_DYNEL_TYPE && crate::ballista::is_lever(object.instance))
                                        .map(|(object, action)| (object.instance, action))
                                {
                                    // pulling a ballista's lever (the Gauntlet's shooting room)
                                    if action != INTERACTION_STOP_USE {
                                        crate::ballista::pull(h.identity, guid);
                                    }
                                    stop_after_success = true;
                                } else if command_type == crate::buildie::COMMAND_OBJECT_INTERACTION
                                    && let Some((guid, action)) = interaction_object(data)
                                        .filter(|&(object, _)| object.kind == crate::scenery::SIMPLE_DYNEL_TYPE && crate::mirrors::is_mirror(object.instance))
                                        .map(|(object, action)| (object.instance, action))
                                {
                                    // turning a Medusa mirror (not again for the stop-use the client queues after it)
                                    if action != INTERACTION_STOP_USE {
                                        crate::mirrors::turn(h.identity, guid);
                                    }
                                    stop_after_success = true;
                                } else if command_type == crate::buildie::COMMAND_OBJECT_INTERACTION
                                    && let Some((portal, action)) = interaction_object(data)
                                        .filter(|&(object, _)| object.kind == crate::teleport::DOOR_TYPE)
                                        .and_then(|(object, action)| Some((crate::teleport::door_portal(object.instance)?, action)))
                                {
                                    // using a portal's door (click or E) opens its window wherever
                                    // the player stands, as live did
                                    if action != INTERACTION_STOP_USE {
                                        portals.use_door(&mut conn, character, portal).await?;
                                    }
                                    stop_after_success = true;
                                } else if command_type == crate::buildie::COMMAND_OBJECT_INTERACTION
                                    && let Some(f) = target.and_then(|(kind, instance)| (kind == crate::scenery::SIMPLE_DYNEL_TYPE).then(|| crate::discovery::collectible(instance)).flatten())
                                {
                                    // using a collectible finds it
                                    let found = crate::achievements::Event::Found { playfield: f.playfield, name: f.name.clone() };
                                    let _ = outbox.send(world::Out::Progress(found));
                                    stop_after_success = true;
                                } else if command_type == crate::buildie::COMMAND_OBJECT_INTERACTION {
                                    // building a buildie; a repeat (already building or built) is
                                    // answered with success and then failure, as live did
                                    stop_after_success = crate::buildie::interact(h.identity, data) == Some(false);
                                } else if spell == Some(POTION_SPELL) {
                                    // Potion of Health: spends a potion (token 150) and heals.
                                    let (gained, spent) = me.tokens.entry(TOKEN_HEALTH_POTIONS).or_default();
                                    if *spent >= *gained {
                                        refused = true;
                                    } else {
                                        *spent += 1;
                                        let args = Writer::new().u32(TOKEN_HEALTH_POTIONS).u64(*gained).u64(*spent).finish();
                                        conn.send_as(Some(character), Endpoint::GameCharInterface, game_char_interface::UPDATE_TOKEN_AMOUNT, args)
                                            .await?;
                                        let max = me.team.first().map_or(START_HEALTH, |&(_, t)| crate::combat::max_life(t, me.upgrade(t), me.level()));
                                        let health = crate::npc::heal_player(h.identity, (max as f32 * POTION_HEAL_SHARE) as u32);
                                        extra.push(int32_stat(h.identity, STAT_HEALTH, health));
                                        // the heal visual (hearts), shown to everyone
                                        let effect = crate::effects::effect_package(h.identity, POTION_EFFECT);
                                        world::send_playfield(h.identity, false, world::Out::Records(vec![effect.clone()]));
                                        extra.push(effect);
                                    }
                                } else if let Some(spell) = spell {
                                    crate::procs::cast(h.identity, spell);
                                    let damage = me
                                        .team
                                        .first()
                                        .map_or(50, |&(_, t)| crate::combat::damage(t, me.upgrade(t), spell, None, me.level()));
                                    // MagicCommand data: u32, u32 spell, u8 0 + ground position, or u8 1.
                                    // Melee attacks target the caster itself and hit what's around it.
                                    let ground = (data.get(8) == Some(&0)).then(|| {
                                        let f = |i: usize| f32::from_be_bytes(data[i..i + 4].try_into().unwrap());
                                        [f(9), f(13), f(17)]
                                    }).filter(|_| data.len() >= 21);
                                    let target_id = target.map(|(kind, instance)| Id32::new(kind, instance));
                                    if crate::npc::is_summon(spell) {
                                        // The client re-sends the cast while the button is held;
                                        // the original server's recharge stopped that. Refuse
                                        // repeats for a moment.
                                        if last_summon.is_some_and(|(s, t): (u32, std::time::Instant)| s == spell && t.elapsed() < SUMMON_RECHARGE) {
                                            refused = true;
                                        } else {
                                            last_summon = Some((spell, std::time::Instant::now()));
                                            let at = ground.unwrap_or_else(|| *pos.lock().unwrap());
                                            crate::npc::summon(h.identity, place, spell, at);
                                        }
                                    } else if let Some(dash) = crate::dash::params(spell) {
                                        // a dash: the server moves the caster (towards the
                                        // clicked point, or along the facing) and deals the hits
                                        let template = me.team.first().map(|&(_, t)| t);
                                        let active = template.map_or(0, |t| me.upgrade(t));
                                        let level = me.level();
                                        let point = target_id
                                            .filter(|t| *t != h.identity)
                                            .and_then(crate::npc::position)
                                            .or(ground);
                                        crate::dash::start(h.identity, place, spell, dash.clone(), pos.clone(), point, heading, move |m| {
                                            template.map_or(50, |t| crate::combat::damage_with_mod(t, active, spell, m, level))
                                        });
                                    } else if crate::projectile::params(spell).is_some_and(|p| !p.charge_items.is_empty()) {
                                        // a charged shot: held, fired on release (BCC action 20)
                                        // at the target or where the player faces then; its hits
                                        // deal the damage
                                        let npc_target = target_id.filter(|t| *t != h.identity && crate::npc::is_npc(*t));
                                        let mut recast = auto_fired.lock().unwrap();
                                        let held_on = recast.is_some_and(|(s, t): (u32, std::time::Instant)| s == spell && t.elapsed() < CHARGE_RECAST_GAP);
                                        if held_on {
                                            // the button is still held after an automatic fire:
                                            // the client re-sends the cast; refuse until it stops
                                            *recast = Some((spell, std::time::Instant::now()));
                                            refused = true;
                                        }
                                        drop(recast);
                                        if !held_on {
                                            fire_stale_charges(&mut charges, &me, h.identity);
                                            if let Some(client_id) = msg.commandqueue_client_id {
                                                let active = Arc::new(std::sync::atomic::AtomicBool::new(true));
                                                let full_damage = me.team.first().map_or(damage, |&(_, t)| {
                                                    crate::combat::damage(t, me.upgrade(t), spell, Some(usize::MAX), me.level())
                                                });
                                                let mut end = blob.to_vec();
                                                end[blob.len() - rest.len()] = BCC_FINISH_CHANNEL;
                                                run_charge(
                                                    h.identity,
                                                    place,
                                                    spell,
                                                    active.clone(),
                                                    pos.clone(),
                                                    facing.clone(),
                                                    npc_target,
                                                    full_damage,
                                                    Bytes::from(end),
                                                    auto_fired.clone(),
                                                );
                                                charges.insert(client_id, Charge {
                                                    spell,
                                                    started: std::time::Instant::now(),
                                                    active,
                                                    target: npc_target,
                                                    damage,
                                                    place,
                                                });
                                            }
                                        }
                                    } else if crate::actions::is_channel(spell) {
                                        // a channelled ability: the server runs its action lists
                                        // until the release (BCC action 20) or the last tick
                                        let (template, upgrades) = me.team.first().map_or((0, 0), |&(_, t)| (t, me.upgrade(t)));
                                        let mut end = blob.to_vec();
                                        end[blob.len() - rest.len()] = BCC_FINISH_CHANNEL;
                                        let caster = crate::actions::Caster {
                                            id: h.identity,
                                            playfield: place,
                                            pos: pos.clone(),
                                            facing: facing.clone(),
                                            template,
                                            upgrades,
                                            level: me.level(),
                                            target: target_id,
                                            ground,
                                        };
                                        crate::actions::start(caster, spell, Bytes::from(end));
                                    } else if crate::actions::is_ability(spell) {
                                        // an attack with server-side action lists (combo buffs,
                                        // area formulas and their hit effects): run as live did,
                                        // hits at the spell's ItemDelay
                                        let (template, upgrades) = me.team.first().map_or((0, 0), |&(_, t)| (t, me.upgrade(t)));
                                        let caster = crate::actions::Caster {
                                            id: h.identity,
                                            playfield: place,
                                            pos: pos.clone(),
                                            facing: facing.clone(),
                                            template,
                                            upgrades,
                                            level: me.level(),
                                            target: target_id,
                                            ground,
                                        };
                                        crate::actions::cast(caster, spell);
                                        can_execute_after = crate::actions::hit_delay(spell);
                                    } else if crate::projectile::params(spell).is_some() {
                                        // a shot the action lists don't cover: aim at the target,
                                        // the clicked point or where the player last moved
                                        let here = *pos.lock().unwrap();
                                        let npc_target = target_id.filter(|t| *t != h.identity && crate::npc::is_npc(*t));
                                        let towards = npc_target
                                            .and_then(|t| crate::projectile::aim_for(spell, t, here))
                                            .or(ground)
                                            .unwrap_or([here[0] + heading[0], here[1], here[2] + heading[1]]);
                                        crate::projectile::launch(h.identity, place, spell, None, here, towards, npc_target, damage);
                                    } else {
                                    match target_id {
                                        Some(t) if t != h.identity && crate::npc::is_npc(t) => {
                                            crate::npc::hit(h.identity, t, spell, damage);
                                        }
                                        _ => {
                                            // melee: in front of the character (its facing)
                                            let here = *pos.lock().unwrap();
                                            let ahead = [here[0] + heading[0] * MELEE_REACH, here[1], here[2] + heading[1] * MELEE_REACH];
                                            let center = ground.unwrap_or(ahead);
                                            crate::npc::hit_area(h.identity, place, center, AREA_RADIUS, spell, damage);
                                        }
                                    }
                                    }
                                }
                            }
                            None => info!(command_type, rest = %crate::conn::hex(rest), "command (unparsed)"),
                        }
                        let action_at = blob.len() - rest.len();
                        let mut reply = blob.to_vec();
                        reply[action_at] = if refused { BCC_COMMAND_FAILED } else { BCC_COMMAND_SUCCEEDED };
                        let mut records = vec![Bytes::from(reply.clone())];
                        if stop_after_success {
                            let mut failed = reply.clone();
                            failed[action_at] = BCC_COMMAND_FAILED;
                            records.push(Bytes::from(failed));
                        }
                        records.extend(extra);
                        if command_type == COMMAND_MAGIC
                            && !refused
                            && let Some(spell) = cast_spell
                            && spell != POTION_SPELL
                        {
                            // the others see the cast (its animation) too
                            let target = cast_target.unwrap_or(h.identity);
                            let (observed, records) = crate::npc::observed_cast(h.identity, target, spell);
                            world::send_playfield(h.identity, false, world::Out::Records(records.to_vec()));
                            // the others' copy, ended with the caster's release or abort (held
                            // charges and channels)
                            if let Some(client_id) = cast_client_id {
                                observed_casts.retain(|_, o| o.3.elapsed() < OBSERVED_CAST_KEEP);
                                observed_casts.insert(client_id, (observed, target, spell, std::time::Instant::now()));
                            }
                        }
                        if command_type == COMMAND_MAGIC && !refused {
                            // A MagicCommand only executes (and later finishes, freeing the queue
                            // for the next cast) once the server allows it: action 10 sets
                            // m_bCanExecute (MagicCommand_c::CanExecute, RunCommand 0x136d280).
                            reply[action_at] = BCC_CAN_EXECUTE;
                            match can_execute_after {
                                Some(delay) => {
                                    let outbox = outbox.clone();
                                    let reply = Bytes::from(reply);
                                    tokio::spawn(async move {
                                        tokio::time::sleep(delay).await;
                                        let _ = outbox.send(world::Out::Records(vec![reply]));
                                    });
                                }
                                None => records.push(Bytes::from(reply)),
                            }
                        }
                        if command_type == COMMAND_CYCLE_WEAPON && rest.len() >= 4 {
                            // CycleToNextWeaponCommand: subclass ends with the u32 team slot
                            let slot = u32::from_be_bytes(rest[rest.len() - 4..].try_into().unwrap());
                            if let Some(i) = me.team.iter().position(|&(s, _)| s == slot) {
                                me.team[..=i].rotate_right(1);
                                if std::mem::take(&mut in_limbo) {
                                    // back with a rebuilt minifigure after the whole team was
                                    // smashed: at the nearest respawn point
                                    respawn(h.identity, &me, *pos.lock().unwrap(), None, &outbox);
                                }
                                records.push(int32_stat(h.identity, STAT_ACTIVE_WEAPON, slot));
                                records.extend(active_minifigure_records(h.identity, me.team[0].1, me.upgrade(me.team[0].1), me.level(), &presence));
                                crate::auras::apply(h.identity, me.team[0].1, &me.tags, true);
                            }
                        }
                        if command_type == COMMAND_EQUIP_ITEM
                            && let Some((_, data)) = parse_command(rest)
                            && let Some((slot, template)) = equip(&mut me, &mut collection, data)
                        {
                            // EquipItemCommand: the client moves the item itself on success. Into
                            // the active slot: the new minifigure is active. With the whole team
                            // smashed, an unsmashed one takes over wherever it goes, at the
                            // nearest respawn point.
                            let revives = in_limbo && !me.smashed.contains_key(&template);
                            if revives && let Some(i) = me.team.iter().position(|&(s, _)| s == slot) {
                                me.team[..=i].rotate_right(1);
                                records.push(int32_stat(h.identity, STAT_ACTIVE_WEAPON, 0));
                                records.push(int32_stat(h.identity, STAT_ACTIVE_WEAPON, slot));
                            }
                            if me.team.first() == Some(&(slot, template)) {
                                records.extend(active_minifigure_records(h.identity, template, me.upgrade(template), me.level(), &presence));
                                crate::auras::apply(h.identity, template, &me.tags, true);
                            }
                            if revives {
                                in_limbo = false;
                                respawn(h.identity, &me, *pos.lock().unwrap(), None, &outbox);
                            }
                        }
                        if command_type == COMMAND_EQUIP_ITEM {
                            learn_team_spells(&mut conn, character, &me).await?;
                        }
                        if command_type == COMMAND_BCC_STATE
                            && !refused
                            && crate::buildie::building(h.identity)
                            && let Some((_, data)) = parse_command(rest)
                            && let Some(state) = data.get(0..4).map(|b| u32::from_be_bytes(b.try_into().unwrap()))
                        {
                            // while building, the client asks for BCCState 104 (the build
                            // animation): live set it on the player, for everyone
                            let record = stat_record(h.identity, STAT_BCC_STATE, state);
                            records.push(record.clone());
                            world::send_playfield(h.identity, false, world::Out::Records(vec![record]));
                        }
                        let blob = iir::pass_blob(&records);
                        conn.send_as(Some(character), Endpoint::GameserverToClientRPC, client::PASS_BLOB, blob).await?;
                    }
                    Some("CoreProjectileIIR") => crate::projectile::report(h.identity, blob, rest),
                    Some("KnubotAnswerIIR") => {
                        // u16 1, ID32 npc, u32 answer index
                        if let Some(b) = rest.get(10..14) {
                            crate::knubot::answer(h.identity, u32::from_be_bytes(b.try_into().unwrap()));
                        }
                    }
                    Some("MoveInventoryItemIIR_t") => {
                        // Reordering the team (101 -> 101). The client already moved it locally;
                        // echoing would move it back.
                        let mut pr = Reader::new(rest);
                        let len = pr.u32()? as usize;
                        let mv = <lmoxide_net::proto::ProtoMoveInventoryItemIir as prost::Message>::decode(pr.bytes(len, "move")?)?;
                        let (from, to) = (mv.source_item_pos, mv.destination_item_pos);
                        info!(?mv, "move inventory item");
                        let active_slot = me.team.first().map(|&(s, _)| s);
                        let a = me.team.iter().position(|&(s, _)| s == from);
                        let b = me.team.iter().position(|&(s, _)| s == to);
                        match (a, b) {
                            (Some(a), Some(b)) => {
                                let (ta, tb) = (me.team[a].1, me.team[b].1);
                                me.team[a].1 = tb;
                                me.team[b].1 = ta;
                            }
                            (Some(a), None) => me.team[a].0 = to,
                            _ => {}
                        }
                        // keep the active slot first
                        if let Some(i) = me.team.iter().position(|&(s, _)| Some(s) == active_slot) {
                            me.team[..=i].rotate_right(1);
                        }
                        if active_slot.is_some_and(|s| s == from || s == to)
                            && let Some(&(_, template)) = me.team.first()
                        {
                            let records = active_minifigure_records(h.identity, template, me.upgrade(template), me.level(), &presence);
                            crate::auras::apply(h.identity, template, &me.tags, true);
                            conn.send_as(Some(character), Endpoint::GameserverToClientRPC, client::PASS_BLOB, iir::pass_blob(&records))
                                .await?;
                        }
                    }
                    Some("QuestCooldownIIR") => {
                        // the client asks for the cooldowns every ~15 s (u32 4, empty list); live
                        // answered with one record per quest cooling down
                        send_records(&mut conn, character, crate::quests::cooldowns(&me, character)).await?;
                    }
                    _ => info!(iir = h.name().unwrap_or("?"), identity = ?h.identity, rest = %crate::conn::hex(rest), "client IIR"),
                }
            }
            (Some(Endpoint::ClientToGameserverRPC), agent::REQUEST_RESURRECTION_SPOTS) => {
                // RequestResurrectionSpots(InstanceId playfield): the World Map shows another world
                let playfield = r.u32()?;
                let args = crate::teleport::signal_spots_args(playfield);
                conn.send_as(Some(character), Endpoint::GameserverToClientRPC, client::SIGNAL_RESURRECTION_SPOTS_RECEIVED, args)
                    .await?;
            }
            (Some(Endpoint::GameCharAgent), game_char_agent::TELEPORT_ME) => {
                // TeleportMe(uint32 playfieldID, uint32 spawnPointID): the world map
                let playfield = r.u32()?;
                let spawn = r.u32()?;
                info!(playfield, spawn, "TeleportMe");
                teleport(&mut conn, &store, character, &mut me, &collection, &mut presence, &pos, crate::instance::towards(place, playfield), &mut place, Some(spawn), None)
                    .await?;
            }
            (Some(Endpoint::GameCharAgent), game_char_agent::CLIENT_REQUEST_DESTINATION_PLAYFIELD_STATUS) => {
                // The teleport window builds its buttons only once this is answered:
                // ReplyToClientRequestDestinationPlayfieldStatus(bool isReady, bool dungeonCompleted,
                // bool playfieldEmpty)
                let args = Writer::new().bool(true).bool(false).bool(true).finish();
                conn.send_as(
                    Some(character),
                    Endpoint::GameCharInterface,
                    game_char_interface::REPLY_TO_CLIENT_REQUEST_DESTINATION_PLAYFIELD_STATUS,
                    args,
                )
                .await?;
            }
            (Some(Endpoint::GameCharAgentProject), game_char_agent_project::UPDATE_SPELL_TARGET_POSITION) => {
                // UpdateSpellTargetPosition(uint32 commandID, Vector3 position): where a channel
                // aims now
                let _command = r.u32()?;
                let at = [r.f32()?, r.f32()?, r.f32()?];
                if let Some(p) = &presence {
                    crate::actions::aim(p.id(), at);
                }
            }
            (Some(Endpoint::GameCharAgentProject), game_char_agent_project::TELEPORT_MESSAGE_BOX_ANSWER) => {
                // TeleportMessageBoxAnswer(int answer, int ctx): the playfield (high bit: "enter
                // existing"), or -1 for cancel
                let answer = r.i32()?;
                info!(answer, "teleport window answer");
                if answer > 0 || answer < -1 {
                    let playfield = (answer as u32) & 0x7fff_ffff;
                    teleport(&mut conn, &store, character, &mut me, &collection, &mut presence, &pos, crate::instance::towards(place, playfield), &mut place, None, None)
                        .await?;
                }
            }
            (Some(Endpoint::GameCharAgent), game_char_agent::SCRY_MESSAGE_BOX_ANSWER) => {
                // ScryMessageBoxAnswer(int answer, int ctx, uint64 cookie): 1 Yes, 2 No or closed
                let answer = r.i32()?;
                let _ctx = r.i32()?;
                let cookie = r.u64()?;
                info!(answer, cookie, "message box answer");
                if let Some((asked, to)) = portals.prompt
                    && asked == cookie
                {
                    portals.prompt = None;
                    if answer == 1 {
                        teleport(&mut conn, &store, character, &mut me, &collection, &mut presence, &pos, crate::instance::towards(place, to), &mut place, None, None).await?;
                    }
                }
            }
            (Some(Endpoint::GameCharAgentProject), game_char_agent_project::TELEPORT_WINDOW_BYPASS) => {
                // TeleportWindowBypass(uint32 windowsId, uint32 playfieldId): the Tutorial's map
                // sends this for Pirate World instead of opening the window
                let window = r.u32()?;
                let playfield = r.u32()?;
                info!(window, playfield, "teleport window bypass");
                teleport(&mut conn, &store, character, &mut me, &collection, &mut presence, &pos, crate::instance::towards(place, playfield), &mut place, None, None)
                    .await?;
            }
            (Some(Endpoint::GameCharAgentProject), game_char_agent_project::CLIENT_ATTEMPT_CLOSING_PLAYER_DESTINATION_PLAYFIELD) => {
                // the teleport window's Reset (dungeon instances): instances aren't kept, so it
                // always succeeds; SignalReplyToClientClosingPlayfield(bool) reloads the window
                let args = Writer::new().bool(true).finish();
                conn.send_as(
                    Some(character),
                    Endpoint::GameCharInterfaceProject,
                    game_char_interface_project::SIGNAL_REPLY_TO_CLIENT_CLOSING_PLAYFIELD,
                    args,
                )
                .await?;
            }
            (Some(Endpoint::GameCharAgentProject), game_char_agent_project::LEVEL_UP_MINIFIGURE) => {
                // LevelUpMinifigure(uint32 minifigure): buy the next rank (upgrade tier) with stars
                let template = r.u32()?;
                let rank = me.rank(template);
                let cost = crate::progression::rank_cost(rank);
                let ok = crate::progression::enabled()
                    && cost.is_some_and(|c| me.stars >= c)
                    && me.level() >= crate::progression::rank_player_level(rank);
                info!(template, rank, ?cost, stars = me.stars, ok, "level up minifigure");
                if ok {
                    me.stars -= cost.unwrap_or(0);
                    me.minifigure_levels.insert(template, rank + 1);
                    // (the client's UI never sends this; upgrades are bought in the Item Shop)
                    let purchased = me.purchased(template) | crate::progression::unlocked_upgrades(rank + 1);
                    me.purchased_upgrades.insert(template, purchased);
                    send_token(&mut conn, character, TOKEN_STARS, me.stars, 0).await?;
                    let (inv, pos) = match me.team.iter().find(|&&(_, t)| t == template) {
                        Some(&(slot, _)) => (crate::collection::TEAM_INVENTORY, slot),
                        None => (crate::collection::COLLECTION_INVENTORY, collection.position_of(template).unwrap_or(0)),
                    };
                    let stats = [
                        (crate::collection::STAT_MINIFIGURE_LEVEL, rank + 1),
                        (crate::collection::STAT_MINIFIGURE_XP, crate::progression::rank_xp(rank + 1).unwrap_or(0) as u32),
                        (crate::collection::STAT_PURCHASED_UPGRADES, purchased),
                    ];
                    for (stat, value) in stats {
                        // SetItemStat(const ID32& inventory, int pos, int stat, int value)
                        let args = Id32::new(inv, character).write(Writer::new()).i32(pos as i32).i32(stat as i32).i32(value as i32).finish();
                        conn.send_as(Some(character), Endpoint::GameCharInterface, game_char_interface::SET_ITEM_STAT, args).await?;
                    }
                    let out = progress(&mut me, &collection, character, Some((None, 0)));
                    send_progress(&mut conn, character, place, &mut me, &mut collection, out).await?;
                }
                let args = Writer::new().u32(template).i32(if ok { 0 } else { 1 }).finish();
                conn.send_as(
                    Some(character),
                    Endpoint::GameCharInterfaceProject,
                    game_char_interface_project::SIGNAL_MINIFIGURE_UPGRADE_RESPONSE,
                    args,
                )
                .await?;
            }
            (Some(Endpoint::GameCharAgentProject), game_char_agent_project::ACTIVATE_MINIFIGURE_UPGRADE) => {
                // ActivateMinifigureUpgrade(int minifigureID, int position)
                let template = r.i32()? as u32;
                let position = r.i32()? as u32;
                let old = me.upgrade(template);
                // only upgrades bought (all without progression)
                let active = crate::collection::activate(old, position) & me.purchased(template);
                me.upgrades.insert(template, active);
                info!(template, position, active, "activate upgrade");
                send_upgrades(&mut conn, character, &me, &collection, template, old, &[(crate::collection::STAT_ACTIVE_UPGRADES, active)]).await?;
                let args = Writer::new().u32(template).i32(0).finish();
                conn.send_as(
                    Some(character),
                    Endpoint::GameCharInterfaceProject,
                    game_char_interface_project::SIGNAL_MINIFIGURE_UPGRADE_RESPONSE,
                    args,
                )
                .await?;
            }
            (Some(Endpoint::GameCharAgentProject), game_char_agent_project::REQUEST_ITEM_SHOP_TRANSACTION) => {
                // RequestItemShopTransaction(requestId, targetTemplateId, itemShopId, costType,
                // amount): 1 at login (the item list), 3 buys an item for a minifigure
                let (request, template, shop_id, currency, amount) = (r.u32()?, r.u32()?, r.u32()?, r.u32()?, r.u32()?);
                info!(request, template, shop_id, currency, amount, "item shop transaction");
                if request == crate::shop::REQUEST_LIST {
                    let args = crate::shop::upgrade_items();
                    conn.send_as(Some(character), Endpoint::GameCharInterfaceProject, game_char_interface_project::UPDATE_ITEM_SHOP_ITEMS, args)
                        .await?;
                } else if request == crate::shop::REQUEST_BUY {
                    let owned = collection.position_of(template).is_some() || me.team.iter().any(|&(_, t)| t == template);
                    let bought = match crate::shop::upgrade_position(shop_id) {
                        _ if !crate::progression::enabled() => Err("Every upgrade is unlocked on this server."),
                        _ if currency != crate::shop::STARS => Err("Upgrades are bought with stars."),
                        _ if !owned => Err("You don't have this minifigure."),
                        None => Err("This item isn't for sale."),
                        Some(position) => crate::shop::check(position, me.purchased(template), me.upgrade(template), me.level(), me.stars)
                            .map(|price| (position, price)),
                    };
                    match bought {
                        Ok((position, price)) => {
                            let old = me.upgrade(template);
                            let purchased = me.purchased(template) | (1 << position);
                            me.stars -= u64::from(price);
                            me.purchased_upgrades.insert(template, purchased);
                            // bought means picked: it's activated (its pair partner deselected)
                            let active = crate::collection::activate(old, position) & purchased;
                            me.upgrades.insert(template, active);
                            // the minifigure's level: one per upgrade bought
                            let rank = purchased.count_ones().min(crate::progression::MAX_RANK);
                            me.minifigure_levels.insert(template, rank);
                            info!(template, position, price, stars = me.stars, purchased, active, rank, "upgrade bought");
                            let stats = [
                                (crate::collection::STAT_PURCHASED_UPGRADES, purchased),
                                (crate::collection::STAT_ACTIVE_UPGRADES, active),
                                (crate::collection::STAT_MINIFIGURE_LEVEL, rank),
                                (crate::collection::STAT_MINIFIGURE_XP, crate::progression::rank_xp(rank).unwrap_or(0) as u32),
                            ];
                            send_upgrades(&mut conn, character, &me, &collection, template, old, &stats).await?;
                            send_token(&mut conn, character, TOKEN_STARS, me.stars, 0).await?;
                            let args = item_shop_currencies(me.stars);
                            conn.send_as(Some(character), Endpoint::GameCharInterfaceProject, game_char_interface_project::ITEM_SHOP_CURRENCIES, args)
                                .await?;
                            // the open upgrade panel refreshes its stars and tiers on this
                            // (MinifigureUpgradePanel.SlotMinifigureUpgradeResponse)
                            let args = Writer::new().u32(template).i32(0).finish();
                            conn.send_as(
                                Some(character),
                                Endpoint::GameCharInterfaceProject,
                                game_char_interface_project::SIGNAL_MINIFIGURE_UPGRADE_RESPONSE,
                                args,
                            )
                            .await?;
                            // ItemShopReply(int methodId, uint32 templateId, uint32 optionalValue)
                            let args = Writer::new().i32(crate::shop::METHOD_BOUGHT).u32(template).u32(0).finish();
                            conn.send_as(Some(character), Endpoint::GameCharInterfaceProject, game_char_interface_project::ITEM_SHOP_REPLY, args)
                                .await?;
                            let out = progress(&mut me, &collection, character, Some((None, 0)));
                            send_progress(&mut conn, character, place, &mut me, &mut collection, out).await?;
                        }
                        Err(why) => {
                            info!(template, shop_id, why, "upgrade not bought");
                            // ItemShopReplyError(int methodId, uint32 templateId, String error)
                            let args = Writer::new().i32(crate::shop::METHOD_ERROR).u32(template).string(why).finish();
                            conn.send_as(Some(character), Endpoint::GameCharInterfaceProject, game_char_interface_project::ITEM_SHOP_REPLY_ERROR, args)
                                .await?;
                        }
                    }
                }
            }
            (Some(Endpoint::ClientToGameserverRPC), agent::PING) => {
                let seq = r.u32()?;
                let args = Writer::new().u32(seq).finish();
                conn.send_as(Some(character), Endpoint::GameserverToClientRPC, client::PONG, args).await?;
            }
            (Some(Endpoint::GameCharAgent), game_char_agent::PING) => {
                // Pong(uint32 i_nSequence, bigtime_t i_nServerTime); server time in microseconds
                let seq = r.u32()?;
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_micros() as u64);
                let args = Writer::new().u32(seq).u64(now).finish();
                conn.send_as(Some(character), Endpoint::GameCharInterface, game_char_interface::PONG, args)
                    .await?;
            }
            (Some(Endpoint::GameCharAgent), game_char_agent::VICINITY_CHAT_MESSAGE) => {
                // VicinityChatMessage(const String& message, const int mood), sent after the chat
                // filter approved it. The client ignores GameCharInterface::VicinityChatMessage;
                // others see the line through their chat-server connection (chat.rs). The
                // sender has already echoed it.
                let message = r.string()?;
                let mood = r.i32()?;
                info!(name = %me.name, %message, mood, "chat");
                if crate::devcmd::is_command(&message) && gm_level < cfg.devcmd_gm_level {
                    crate::chat::system(character, "Test commands need a GM level.");
                } else if crate::devcmd::is_command(&message) {
                    // a test command (devcmd.rs), not relayed
                    let here = *pos.lock().unwrap();
                    match crate::devcmd::run(&message, place, here) {
                        crate::devcmd::Action::Reply(lines) => {
                            for line in lines {
                                crate::chat::system(character, &line);
                            }
                        }
                        crate::devcmd::Action::Xp(xp) => {
                            if crate::progression::enabled() {
                                let records = gain_xp(&mut me, character, xp);
                                conn.send_as(Some(character), Endpoint::GameserverToClientRPC, client::PASS_BLOB, iir::pass_blob(&records)).await?;
                            }
                            let line = format!("level {}, XP {} (next level at {})", me.level(), me.xp, crate::progression::next_xp(me.level()));
                            crate::chat::system(character, &line);
                        }
                        crate::devcmd::Action::Stars(stars) => {
                            me.stars += stars;
                            send_token(&mut conn, character, TOKEN_STARS, me.stars, 0).await?;
                            crate::chat::system(character, &format!("{} stars", me.stars));
                        }
                        crate::devcmd::Action::Mirrors(solve) => {
                            let line = if crate::mirrors::set_solved(me.playfield, solve) {
                                format!("mirrors: {}", if solve { "solved" } else { "reset" })
                            } else {
                                "no mirror puzzle here (Armory 2310, Gauntlet 6300)".to_string()
                            };
                            crate::chat::system(character, &line);
                        }
                        crate::devcmd::Action::Smash => {
                            let here = *pos.lock().unwrap();
                            let d = |p: [f32; 3]| (p[0] - here[0]).hypot(p[2] - here[2]);
                            let nearest = crate::npc::targets_where(place, |p| d(p) < 15.0)
                                .into_iter()
                                .min_by(|a, b| d(a.1).total_cmp(&d(b.1)));
                            match nearest {
                                Some((target, _)) => {
                                    info!(?target, "dev smash");
                                    crate::npc::hit(Id32::new(crate::territory::CHARACTER_TYPE, character), target, 0, 100_000);
                                }
                                None => crate::chat::system(character, "nothing to smash within 15 m"),
                            }
                        }
                        crate::devcmd::Action::Fx(fx) => {
                            let id = Id32::new(crate::territory::CHARACTER_TYPE, character);
                            let ids = crate::devcmd::effect_packages();
                            if let Some((package, instance)) = playing_fx.take() {
                                world::send_playfield(id, true, world::Out::Records(vec![crate::effects::stop_effect_package(id, package, 0, instance)]));
                            }
                            let play = match fx {
                                crate::devcmd::Fx::Play(package, hook, at) => Some((package, hook, at)),
                                crate::devcmd::Fx::Step(step) if !ids.is_empty() => {
                                    let at = playing_fx_at.map_or(0, |i| (i as i64 + i64::from(step)).rem_euclid(ids.len() as i64) as usize);
                                    playing_fx_at = Some(at);
                                    Some((ids[at], String::new(), crate::devcmd::FxAt::Me))
                                }
                                _ => None,
                            };
                            if let Some((package, hook, at)) = play {
                                let instance = crate::effects::new_instance();
                                let here = *pos.lock().unwrap();
                                // the record's identity is always the player (so `!fx stop` ends it)
                                let (record, place) = match at {
                                    crate::devcmd::FxAt::Me => (Some(crate::effects::effect_package_hook(id, package, &hook, "", 0.0, 0, instance)), String::new()),
                                    crate::devcmd::FxAt::Ground => (Some(crate::effects::ground_effect(id, package, here, 0, instance)), " at your feet".into()),
                                    crate::devcmd::FxAt::Near => {
                                        let d = |p: [f32; 3]| (p[0] - here[0]).hypot(p[2] - here[2]);
                                        let nearest = crate::npc::targets_where(place, |p| d(p) < 15.0).into_iter().min_by(|a, b| d(a.1).total_cmp(&d(b.1)));
                                        match nearest {
                                            Some((target, _)) => (Some(crate::effects::effect_package_on(id, target, package, &hook, instance)), format!(" on {}", crate::npc::name_of(target).unwrap_or_default())),
                                            None => (None, String::new()),
                                        }
                                    }
                                };
                                match record {
                                    Some(record) => {
                                        world::send_playfield(id, true, world::Out::Records(vec![record]));
                                        playing_fx = Some((package, instance));
                                        let n = ids.iter().position(|&p| p == package).map_or(String::new(), |i| format!(" ({} of {})", i + 1, ids.len()));
                                        let hook = if hook.is_empty() { String::new() } else { format!(" from {hook}") };
                                        crate::chat::system(character, &format!("effect package {package}{n}{hook}{place}"));
                                    }
                                    None => crate::chat::system(character, "no enemy or smashable within 15 m for @near"),
                                }
                            }
                        }
                        crate::devcmd::Action::Despawn(all) => {
                            let here = *pos.lock().unwrap();
                            let n = crate::npc::despawn_tests(place, (!all).then_some((here, 30.0)));
                            crate::chat::system(character, &format!("{n} test spawns removed"));
                        }
                        crate::devcmd::Action::Spawn(name, count) => {
                            let at = *pos.lock().unwrap();
                            let n = crate::npc::spawn_test_enemies(&name, count, place, at, TEST_ENEMY_LIFETIME);
                            let line = if n == 0 { format!("no enemy or smashable named {name}") } else { format!("{n} x {name} for 15 minutes") };
                            crate::chat::system(character, &line);
                        }
                        crate::devcmd::Action::Window(window) => portals.open_window(&mut conn, character, window).await?,
                        crate::devcmd::Action::Bounce(how) => {
                            let id = Id32::new(crate::territory::CHARACTER_TYPE, character);
                            let here = *pos.lock().unwrap();
                            let handle = pos.clone();
                            crate::chat::system(character, &format!("bounce {how:?}"));
                            tokio::spawn(async move { crate::jumppad::throw_as(id, &handle, [here[0] + 6.0, here[1], here[2]], 2.0, how).await });
                        }
                        crate::devcmd::Action::State(state) => {
                            let id = Id32::new(crate::territory::CHARACTER_TYPE, character);
                            world::send_playfield(id, true, world::Out::Records(vec![stat_record(id, STAT_BCC_STATE, state)]));
                            crate::chat::system(character, &format!("BCCState {state}"));
                        }
                        crate::devcmd::Action::Skin(look) => {
                            let id = Id32::new(crate::territory::CHARACTER_TYPE, character);
                            let look = look.unwrap_or_else(|| monster_data_for(me.team.first().map_or(0, |&(_, t)| t)));
                            set_costume(id, look);
                            world::send_playfield(id, true, world::Out::Records(vec![int32_stat(id, STAT_MONSTER_DATA, look)]));
                            crate::chat::system(character, &format!("look {look}"));
                        }
                        crate::devcmd::Action::Server(to) => match crate::switch::request(&to) {
                            Ok((line, peer)) => {
                                if let Err(e) = store.save(character, &me) {
                                    warn!(character, "save failed: {e:#}");
                                }
                                crate::chat::system(character, &line);
                                // A teleport to where the character is saved there (the client only
                                // takes a new world while teleporting), then the client leaves this
                                // game server and authenticates there with the same character and
                                // cookie (live's move between playfield servers). Its chat
                                // connection, closed, comes back to the chat server it knows, which
                                // passes it on (switch.rs).
                                let (playfield, at) = crate::switch::saved_there(character).unwrap_or((cfg.start_playfield, START_POS));
                                let id = Id32::new(crate::territory::CHARACTER_TYPE, character);
                                let mut records = crate::knubot::end(id);
                                records.extend([
                                    crate::teleport::intended_teleport_record(id, crate::instance::arrival(playfield)),
                                    crate::teleport::teleport_record(id, crate::instance::arrival(playfield), at, 0.0),
                                ]);
                                conn.send_as(Some(character), Endpoint::GameserverToClientRPC, client::PASS_BLOB, iir::pass_blob(&records)).await?;
                                crate::switch::set_away(account_id, true);
                                let to = Writer::new().string(&peer.game_addr).finish();
                                conn.send_as(Some(character), Endpoint::GameserverToClientRPC, client::REDIRECT_TO_SERVER, to).await?;
                                info!(character, to = %peer.name, "redirected to the peer server");
                                tokio::spawn(async move {
                                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                                    crate::chat::disconnect(character);
                                });
                            }
                            Err(line) => crate::chat::system(character, &line),
                        },
                        crate::devcmd::Action::SmashMe => crate::npc::smash_player(Id32::new(crate::territory::CHARACTER_TYPE, character)),
                        crate::devcmd::Action::Loot(kind) => {
                            let id = Id32::new(crate::territory::CHARACTER_TYPE, character);
                            let here = *pos.lock().unwrap();
                            let items = crate::loot::test_items(&kind, place);
                            crate::chat::system(character, &format!("loot {kind}: {} items", items.len()));
                            crate::loot::drop_items(id, place, [here[0] + 1.5, here[1], here[2]], id, items);
                        }
                        crate::devcmd::Action::Own(None) => {
                            let line = match &me.collection {
                                Some(owned) => format!("you own {} of {} minifigures", owned.len(), crate::collection::all_minifigures().len()),
                                None => "you own every minifigure (!collection limit to collect them)".into(),
                            };
                            crate::chat::system(character, &line);
                        }
                        crate::devcmd::Action::Own(Some(template)) => {
                            let line = if grant_minifigure(&mut conn, character, &mut me, &mut collection, template).await? {
                                "obtained"
                            } else {
                                "already owned"
                            };
                            crate::chat::system(character, line);
                        }
                        crate::devcmd::Action::LimitCollection(limit) => {
                            let now = crate::smash::now_ms() / 1000;
                            me.collection = limit.then(|| {
                                me.team.iter().map(|&(_, t)| t).chain(crate::loot::membership_minifigures().iter().copied()).map(|t| (t, now)).collect()
                            });
                            collection = Collection::new(&me.team, me.collection.as_ref());
                            if let Err(e) = store.save(character, &me) {
                                warn!(character, "save failed: {e:#}");
                            }
                            crate::chat::system(character, if limit { "collection limited to the team; relog to see it" } else { "every minifigure owned; relog to see it" });
                        }
                        crate::devcmd::Action::Team(name) => {
                            let answer = match crate::community::team_up(character, &name) {
                                Ok(mate) => format!("in a team with {mate}"),
                                Err(e) => e,
                            };
                            crate::chat::system(character, &answer);
                        }
                        crate::devcmd::Action::Revive => {
                            // the server's health of each minifigure, then the client's: the
                            // MinifigureHealth item stat it switches to (a smashed one has 0) and
                            // the active one's Life and Health
                            let id = Id32::new(crate::territory::CHARACTER_TYPE, character);
                            crate::npc::revive_player(id);
                            for template in std::mem::take(&mut me.smashed).into_keys() {
                                let args = crate::smash::unlock_args(template);
                                conn.send_as(Some(character), Endpoint::GameCharInterface, game_char_interface::UNLOCK_TEMPLATE, args).await?;
                            }
                            if std::mem::take(&mut in_limbo) {
                                world::send_playfield(id, true, world::Out::Records(vec![stat_record(id, STAT_BCC_STATE, BCC_STATE_NORMAL)]));
                            }
                            let items = me.team.iter().map(|&(p, t)| (crate::collection::TEAM_INVENTORY, p, t)).chain(
                                collection.slots.iter().enumerate().filter_map(|(p, t)| t.map(|t| (crate::collection::COLLECTION_INVENTORY, p as u32, t))),
                            );
                            let mut n = 0;
                            for (inventory, p, t) in items.collect::<Vec<_>>() {
                                let (_, full) = crate::combat::item_life(t, me.upgrade(t), me.level());
                                let args = Id32::new(inventory, character)
                                    .write(Writer::new())
                                    .i32(p as i32)
                                    .i32(crate::collection::STAT_MINIFIGURE_HEALTH as i32)
                                    .i32(full as i32)
                                    .finish();
                                conn.send_as(Some(character), Endpoint::GameCharInterface, game_char_interface::SET_ITEM_STAT, args).await?;
                                n += 1;
                            }
                            if let Some(&(_, t)) = me.team.first() {
                                let records = health_records(id, t, me.upgrade(t), me.level());
                                conn.send_as(Some(character), Endpoint::GameserverToClientRPC, client::PASS_BLOB, iir::pass_blob(&records)).await?;
                            }
                            info!(character, minifigures = n, "dev revive");
                            crate::chat::system(character, &format!("{n} minifigures revived"));
                        }
                        crate::devcmd::Action::RemoveAchievements(which) => {
                            let _ = outbox.send(world::Out::RemoveAchievements(which, std::sync::Arc::new(std::sync::Mutex::new(None))));
                        }
                        crate::devcmd::Action::Quests(reset) => {
                            if reset {
                                me.quests.clear();
                                // (the client forgets them with the next playfield)
                                quest_tracker = crate::quests::Tracker::default();
                                crate::chat::system(character, "quest progress and cooldowns cleared; change playfield (or relog) so the client forgets them too");
                            }
                            for line in crate::quests::summary(&me) {
                                crate::chat::system(character, &line);
                            }
                        }
                        crate::devcmd::Action::Achievements => {
                            let (earned, total) = crate::achievements::summary(&me);
                            let count = |stat: u32| me.counters.get(&stat).and_then(|m| m.get(&0)).copied().unwrap_or(0);
                            let line = format!("{earned} of {total} achievements; {} kills, {} smashes", count(3_000_116), count(3_000_126));
                            crate::chat::system(character, &line);
                            // the journal: most defeated, places, finds
                            let mut defeats: Vec<_> = me.defeats.iter().collect();
                            defeats.sort_by(|a, b| b.1.count.cmp(&a.1.count));
                            let top: Vec<String> = defeats.iter().take(8).map(|(n, d)| format!("{n} x{}", d.count)).collect();
                            let line = format!("defeated {} kinds: {}", defeats.len(), top.join(", "));
                            crate::chat::system(character, &line);
                            let finds: Vec<&str> = me.finds.keys().map(|k| k.as_str()).collect();
                            let line = format!("visited {} playfields; found {}: {}", me.visits.len(), finds.len(), finds.join(", "));
                            crate::chat::system(character, &line);
                        }
                        crate::devcmd::Action::Progress(event @ crate::achievements::Event::Defeated { .. }, 1) => {
                            // one defeat (!defeat): as a real one (XP, tutorial boss, clears)
                            crate::chat::system(character, &format!("defeated: {event:?}"));
                            let _ = outbox.send(world::Out::Progress(event));
                        }
                        crate::devcmd::Action::Progress(event, n) => {
                            let before = me.tags.len();
                            let out = progress(&mut me, &collection, character, Some((Some(&event), n as u64)));
                            send_progress(&mut conn, character, place, &mut me, &mut collection, out).await?;
                            let records = crate::quests::on_event(&mut quest_tracker, &mut me, character, &event, n as u64);
                            send_records(&mut conn, character, records).await?;
                            let line = format!("counted {n} x {event:?}: {} new achievements", me.tags.len() - before);
                            crate::chat::system(character, &line);
                        }
                        crate::devcmd::Action::Teleport(playfield, at, rot, line) => {
                            crate::chat::system(character, &line);
                            let rot = if rot.is_nan() { heading[0].atan2(heading[1]) } else { rot };
                            teleport(&mut conn, &store, character, &mut me, &collection, &mut presence, &pos, crate::instance::towards(place, playfield), &mut place, None, Some((at, rot)))
                                .await?;
                        }
                        crate::devcmd::Action::Enter(to, at, rot, line) => {
                            crate::chat::system(character, &line);
                            // (!move, !moveto: keep facing the way the player does)
                            let rot = if rot.is_nan() { heading[0].atan2(heading[1]) } else { rot };
                            teleport(&mut conn, &store, character, &mut me, &collection, &mut presence, &pos, to, &mut place, None, Some((at, rot))).await?;
                        }

                        crate::devcmd::Action::Dialog(n) => {
                            let me_id = Id32::new(crate::territory::CHARACTER_TYPE, character);
                            match crate::knubot::dialogues().get(n.wrapping_sub(1)) {
                                Some(d) => {
                                    // the speaker: that NPC if it's here, else the player
                                    let owner = crate::npc::find(&d.npc, place).unwrap_or(me_id);
                                    crate::knubot::start(me_id, &me.name, owner, d.clone());
                                }
                                None => {
                                    let list: Vec<String> = crate::knubot::dialogues().iter().enumerate().map(|(i, d)| format!("{} {} ({})", i + 1, d.npc, d.playfield)).collect();
                                    crate::chat::system(character, &format!("dialogues: {}", list.join(", ")));
                                }
                            }
                        }
                        crate::devcmd::Action::Aura(n) => {
                            if let Some(&(_, template)) = me.team.first() {
                                let tags = crate::auras::tags(template);
                                for (i, tag) in tags.iter().enumerate() {
                                    if i < n as usize {
                                        me.tags.entry(*tag).or_insert(0);
                                    } else {
                                        me.tags.remove(tag);
                                    }
                                }
                                crate::auras::apply(Id32::new(crate::territory::CHARACTER_TYPE, character), template, &me.tags, false);
                                let line = if tags.is_empty() { "this minifigure has no aura".to_string() } else { format!("aura tags {n} of 4 ({tags:?})") };
                                crate::chat::system(character, &line);
                            }
                        }
                        crate::devcmd::Action::Test(setup) => {
                            crate::chat::system(character, &format!("test {}", setup.label));
                            let id = Id32::new(crate::territory::CHARACTER_TYPE, character);
                            let old = me.team.clone();
                            if !setup.team.is_empty() {
                                set_team(&mut me, &mut collection, &setup.team);
                                if setup.destination.is_some() {
                                    // the world is sent again there, and the client's HUD then
                                    // takes the first slot as the active one: as every teleport
                                    active_at_first_position(&mut me.team);
                                }
                                // the HUD's portraits follow only items removed from and added to
                                // the team (RemoveItem, then AddItem as at character creation):
                                // the slots that changed
                                let inventory = Id32::new(crate::collection::TEAM_INVENTORY, character);
                                for &(s, _) in old.iter().filter(|e| !me.team.contains(e)) {
                                    let args = inventory.write(Writer::new()).i32(s as i32).i32(1).finish();
                                    conn.send_as(Some(character), Endpoint::GameCharInterface, game_char_interface::REMOVE_ITEM, args).await?;
                                }
                                team_salt += 1 << 33;
                                let joined: Vec<(u32, u32)> = me.team.iter().copied().filter(|e| !old.contains(e)).collect();
                                let team = crate::collection::team_refresh(character, &joined, &|t| me.item_state(t), team_salt);
                                for e in &team.items {
                                    let args = inventory.write(Writer::new()).i32(e.pos as i32).proto(&e.item).u8(0).finish();
                                    conn.send_as(Some(character), Endpoint::GameCharInterface, game_char_interface::ADD_ITEM, args).await?;
                                }
                                learn_team_spells(&mut conn, character, &me).await?;
                            }
                            let (_, at) = match setup.destination {
                                Some((playfield, at, rot)) => {
                                    teleport(&mut conn, &store, character, &mut me, &collection, &mut presence, &pos, crate::instance::towards(place, playfield), &mut place, None, Some((at, rot)))
                                        .await?;
                                    (playfield, at)
                                }
                                None => {
                                    // staying here: the team changes in place, as an equip does (the
                                    // world isn't sent again, so every minifigure keeps its slot)
                                    if let Some(&(slot, template)) = me.team.first()
                                        && old.first() != me.team.first()
                                    {
                                        let mut records = vec![int32_stat(id, STAT_ACTIVE_WEAPON, 0), int32_stat(id, STAT_ACTIVE_WEAPON, slot)];
                                        records.extend(active_minifigure_records(id, template, me.upgrade(template), me.level(), &presence));
                                        crate::auras::apply(id, template, &me.tags, true);
                                        conn.send_as(Some(character), Endpoint::GameserverToClientRPC, client::PASS_BLOB, iir::pass_blob(&records)).await?;
                                    }
                                    if let Err(e) = store.save(character, &me) {
                                        warn!(character, "saving character failed: {e:#}");
                                    }
                                    (me.playfield, *pos.lock().unwrap())
                                }
                            };
                            for (name, count) in &setup.spawn {
                                let n = crate::npc::spawn_test_enemies(name, *count, place, at, TEST_ENEMY_LIFETIME);
                                let line = if n == 0 { format!("no enemy named {name}") } else { format!("{n} x {name} for 15 minutes") };
                                crate::chat::system(character, &line);
                            }
                        }
                    }
                } else if let Some(p) = &presence {
                    for other in world::neighbours(p.id()) {
                        crate::chat::vicinity(other.instance, character, &me.name, mood, &message);
                    }
                    if cfg.bot {
                        // the test bot answers, so incoming chat can be seen with one client
                        crate::chat::vicinity(character, BOT_INSTANCE, "Bot", 0, &format!("you said: {message}"));
                    }
                }
            }
            (Some(Endpoint::CSPlayerAgent), method) if crate::community::handle_cs(character, method, &mut r)? => {}
            (Some(Endpoint::GameCharAgentProject), game_char_agent_project::REQUEST_MY_FRIEND_KEY) => {
                crate::community::my_friend_code(character);
            }
            (Some(Endpoint::GameCharAgentProject), game_char_agent_project::REQUEST_ADD_FRIEND_WITH_KEY) => {
                crate::community::add_friend_with_code(character, &r.string()?);
            }
            (Some(Endpoint::GameCharAgent), game_char_agent::LEAVE_TEAM) => crate::community::leave(character),
            (Some(Endpoint::GameCharAgent), game_char_agent::KICK_FROM_TEAM) => {
                crate::community::kick(character, Id32::read(&mut r)?.instance);
            }
            (Some(Endpoint::GameCharAgent), game_char_agent::REQUEST_TEAM_MEMBER_POS_LIST) => {
                crate::community::member_positions(character, &mut r)?;
            }
            (Some(Endpoint::GameCharAgent), game_char_agent::TRY_LOOT_ITEM) => {
                // TryLootItem(const ID32& bag, uint32 itemPos, const ID32& char, uint32 desiredPos):
                // the client auto-collected a star or orb, or the player clicked a part, minifigure
                // bag or chest.
                let bag = Id32::read(&mut r)?;
                let item_pos = r.u32()?;
                let id = Id32::new(crate::territory::CHARACTER_TYPE, character);
                let Some(taken) = crate::loot::take(id, bag, item_pos) else { continue };
                match crate::loot::reward(taken.template) {
                    crate::loot::Reward::Stars(n) => {
                        me.stars += n;
                        send_token(&mut conn, character, TOKEN_STARS, me.stars, 0).await?;
                    }
                    crate::loot::Reward::HealthOrb => {
                        let max = me.team.first().map_or(START_HEALTH, |&(_, t)| crate::combat::max_life(t, me.upgrade(t), me.level()));
                        let health = crate::npc::heal_player(id, (max as f32 * HEALTH_ORB_HEAL_SHARE).round() as u32);
                        let blob = iir::pass_blob(&[int32_stat(id, STAT_HEALTH, health)]);
                        conn.send_as(Some(character), Endpoint::GameserverToClientRPC, client::PASS_BLOB, blob).await?;
                    }
                    crate::loot::Reward::Part(token) => {
                        let (gained, spent) = me.tokens.entry(token).or_default();
                        *gained += 1;
                        let (gained, spent) = (*gained, *spent);
                        send_token(&mut conn, character, token, gained, spent).await?;
                    }
                    crate::loot::Reward::Potions(n) => {
                        let (gained, spent) = me.tokens.entry(TOKEN_HEALTH_POTIONS).or_default();
                        *gained += n;
                        let (gained, spent) = (*gained, *spent);
                        send_token(&mut conn, character, TOKEN_HEALTH_POTIONS, gained, spent).await?;
                    }
                    crate::loot::Reward::MinifigureBag(minifigure) => {
                        // The bag reveal (BagOpenEvent.swf): AddItem(ID32 inventory 110002, i32 pos -1,
                        // entry, u8 split) only signals the bag (pos -1 isn't stored); then the
                        // minifigure is added to the collection, or a duplicate pays out stars.
                        let entry = ProtoInventoryGameEntry { item_id: Some(u64::from(taken.template)), item_data: Some(acg_item(taken.template, 1)), ..Default::default() };
                        let args = Id32::new(MINIFIGURE_BAG_INVENTORY, character).write(Writer::new()).i32(-1).proto(&entry).u8(0).finish();
                        conn.send_as(Some(character), Endpoint::GameCharInterface, game_char_interface::ADD_ITEM, args).await?;
                        if !grant_minifigure(&mut conn, character, &mut me, &mut collection, minifigure).await? {
                            me.stars += DUPLICATE_BAG_STARS;
                            send_token(&mut conn, character, TOKEN_STARS, me.stars, 0).await?;
                            info!(character, minifigure, "minifigure bag (duplicate)");
                        }
                    }
                    crate::loot::Reward::Chest { member } => crate::loot::open_chest(id, taken.playfield, taken.pos, bag, member),
                    crate::loot::Reward::Nothing => {}
                }
            }
            (Some(Endpoint::GameCharAgentProject), game_char_agent_project::CONVERT_MINIFIGURE_PARTS) => {
                // "Use duplicate parts to convert to stars" (MinifigureHolder::CalculateSpareParts):
                // every part of an owned minifigure is spare; of one not owned yet, all but one
                // (kept to build it). Answer 1 = done.
                let mut converted = 0;
                for (minifigure, token) in crate::loot::all_part_tokens() {
                    let keep = u64::from(me.collection.as_ref().is_some_and(|o| !o.contains_key(&minifigure)));
                    if let Some((gained, spent)) = me.tokens.get_mut(&token)
                        && *gained > *spent + keep
                    {
                        converted += *gained - *spent - keep;
                        *spent = *gained - keep;
                        let (g, s) = (*gained, *spent);
                        send_token(&mut conn, character, token, g, s).await?;
                    }
                }
                me.stars += converted * STARS_PER_PART;
                send_token(&mut conn, character, TOKEN_STARS, me.stars, 0).await?;
                info!(character, converted, "parts converted to stars");
                let args = Writer::new().u32(1).finish();
                conn.send_as(Some(character), Endpoint::GameCharInterfaceProject, game_char_interface_project::SIGNAL_CONVERT_MINIFIGURE_PARTS_RESPONSE, args)
                    .await?;
            }
            (Some(Endpoint::GameCharAgentProject), game_char_agent_project::BUILDMINIFIGURE_FROM_PARTS) => {
                // one of each part; 0 = built, into the collection (`--progression` characters own
                // only what they collected)
                let template = r.u32()?;
                let tokens = crate::loot::minifigure_part_tokens(template);
                let complete = !tokens.is_empty() && tokens.iter().all(|t| me.tokens.get(t).is_some_and(|(g, s)| g > s));
                let complete = complete && me.collection.as_ref().is_none_or(|o| !o.contains_key(&template));
                if complete {
                    for t in tokens {
                        let (gained, spent) = me.tokens.get_mut(t).unwrap();
                        *spent += 1;
                        let (g, s) = (*gained, *spent);
                        send_token(&mut conn, character, *t, g, s).await?;
                    }
                    grant_minifigure(&mut conn, character, &mut me, &mut collection, template).await?;
                }
                let args = Writer::new().u32(if complete { 0 } else { 1 }).finish();
                conn.send_as(Some(character), Endpoint::GameCharInterfaceProject, game_char_interface_project::SIGNAL_BUILDMINIFIGURE_FROM_PARTS_RESPONSE, args)
                    .await?;
            }
            (Some(Endpoint::GameCharAgentProject), game_char_agent_project::REQUEST_ITEM_SHOP_DATA) => {
                // Live answered every login with the currencies; the items (shop.rs: the minifigure
                // upgrades) price the Collection's upgrade popup.
                let args = item_shop_currencies(me.stars);
                conn.send_as(Some(character), Endpoint::GameCharInterfaceProject, game_char_interface_project::ITEM_SHOP_CURRENCIES, args)
                    .await?;
                let args = crate::shop::upgrade_items();
                conn.send_as(Some(character), Endpoint::GameCharInterfaceProject, game_char_interface_project::UPDATE_ITEM_SHOP_ITEMS, args)
                    .await?;
            }
            (Some(Endpoint::GameCharAgentProject), game_char_agent_project::SET_CHARACTER_CREATION_DATA) => {
                // SetCharacterCreationData(CreatePlayerCharacter: nick, first, last name, u32 set):
                // what the player picked in the launcher, sent by the creation script
                let (nick, first, last) = (r.string()?, r.string()?, r.string()?);
                let set = r.u32()?;
                let names_ok = [&nick, &first, &last].iter().all(|n| n.chars().count() <= 32) && !nick.trim().is_empty();
                let starter = crate::tutorial::starter(set).filter(|_| names_ok && me.playfield == crate::tutorial::CREATION_PLAYFIELD);
                info!(character, %nick, %first, %last, set, ok = starter.is_some(), "SetCharacterCreationData");
                let result = if starter.is_some() { 0 } else { 1 };
                let id = Id32::new(crate::territory::CHARACTER_TYPE, character);
                if let Some(template) = starter {
                    me.name = [nick.trim(), first.trim(), last.trim()].iter().filter(|n| !n.is_empty()).copied().collect::<Vec<_>>().join(" ");
                    if let Some(owned) = me.collection.as_mut() {
                        owned.insert(template, crate::smash::now_ms() / 1000);
                    }
                    set_team(&mut me, &mut collection, &[template]);
                    // (the placeholder team isn't owned)
                    collection = Collection::new(&me.team, me.collection.as_ref());
                    team_changed = true;
                    // the minifigure into the team, as live: AddItem(ID32 inventory, i32 pos, entry,
                    // u8 split), which also gives the HUD its portrait
                    team_salt += 1 << 33;
                    let team = crate::collection::team_refresh(character, &me.team, &|t| me.item_state(t), team_salt);
                    for e in &team.items {
                        let args = Id32::new(crate::collection::TEAM_INVENTORY, character).write(Writer::new()).i32(e.pos as i32).proto(&e.item).u8(0).finish();
                        conn.send_as(Some(character), Endpoint::GameCharInterface, game_char_interface::ADD_ITEM, args).await?;
                    }
                    let record = crate::tutorial::set_name_record(id, nick.trim());
                    conn.send_as(Some(character), Endpoint::GameserverToClientRPC, client::PASS_BLOB, iir::pass_blob(&[record])).await?;
                }
                let args = Writer::new().i32(result).finish();
                conn.send_as(Some(character), Endpoint::GameCharInterfaceProject, game_char_interface_project::SIGNAL_CREATE_PLAYER_CHARACTER_RESULT, args)
                    .await?;
                if starter.is_some() {
                    // on to the tutorial, without progression to the start playfield (the teleport
                    // sends the character again, with its team)
                    conn.send_as(Some(character), Endpoint::GameCharInterface, game_char_interface::TELEPORT_STARTED, Bytes::new()).await?;
                    let (playfield, at) =
                        if crate::progression::enabled() { (crate::tutorial::PLAYFIELD, crate::tutorial::START) } else { (cfg.start_playfield, START_POS) };
                    teleport(&mut conn, &store, character, &mut me, &collection, &mut presence, &pos, crate::instance::towards(place, playfield), &mut place, None, Some((at, 0.0)))
                        .await?;
                }
            }
            (Some(Endpoint::GameCharAgent), game_char_agent::RESPAWN_ME) => {
                // RespawnMe(uint32 spawn point): "Continue" in the smashed window (the whole team
                // smashed). With a team member rebuilt the player respawns there with it; else
                // the window opens again: wait for a rebuild, or equip a minifigure from the
                // collection (which ends it too)
                let spawn = r.u32()?;
                let id = Id32::new(crate::territory::CHARACTER_TYPE, character);
                let ready = me.team.iter().enumerate().filter(|(_, (_, t))| !me.smashed.contains_key(t)).min_by_key(|(_, (slot, _))| *slot).map(|(i, _)| i);
                info!(character, spawn, ready = ready.is_some(), "RespawnMe");
                if let Some(i) = ready {
                    me.team[..=i].rotate_right(1);
                    in_limbo = false;
                    respawn(id, &me, *pos.lock().unwrap(), crate::teleport::spot(me.playfield, spawn), &outbox);
                } else {
                    let first = me.smashed.values().min().map_or(0, |&until| crate::smash::left(until).as_secs() + 1);
                    crate::chat::system(character, &format!("Your minifigures are still smashed: the first is rebuilt in {first} s. Equip another minifigure from your collection, or wait."));
                    conn.send_as(Some(character), Endpoint::GameCharInterface, game_char_interface::RESURRECTION_FAILED_REOPEN_DIALOG, Bytes::new()).await?;
                }
            }
            (Some(Endpoint::GameCharAgent), game_char_agent::TUTORIAL_SHOWN) => {
                // TutorialShown(uint32 tag): a tutorial tip seen (or skipped outside the
                // tutorial). Live answered AddTag(tag, 0) and kept it, so it isn't shown again.
                let tag = r.u32()?;
                if !me.tags.contains_key(&tag) {
                    let out = progress(&mut me, &collection, character, Some((Some(&crate::achievements::Event::Tag(tag)), 1)));
                    send_progress(&mut conn, character, place, &mut me, &mut collection, out).await?;
                }
            }
            // Client-side bookkeeping that needs no answer (to be persisted later).
            (
                Some(Endpoint::GameCharAgent),
                game_char_agent::ADD_SHORTCUT
                | game_char_agent::REMOVE_SHORTCUT
                | game_char_agent::SET_PLAYER_GUI_LAYOUT_MODE,
            ) => {}
            // Sent at every login; the recorded session shows no answer to any of them (the
            // character had no guild).
            (
                Some(Endpoint::GameCharAgent),
                game_char_agent::COMMAND_QUEUE_SIZE_CHANGED | game_char_agent::SET_HIDE_INSPECT_OPTION | game_char_agent::REQUEST_GEAR_BUILD_LOAD,
            )
            | (Some(Endpoint::CSPlayerAgent), cs_player_agent::CLIENT_REQUEST_GUILD_INFO | cs_player_agent::GUILD_REQUEST_MEMBER_LIST) => {}
            _ => warn!(args = %crate::conn::hex(&rpc.args), "unhandled"),
        }
    }
    drop(community);
    if character != 0 {
        crate::npc::player_left(Id32::new(crate::territory::CHARACTER_TYPE, character));
        crate::auras::left(Id32::new(crate::territory::CHARACTER_TYPE, character));
        crate::knubot::left(Id32::new(crate::territory::CHARACTER_TYPE, character));
        crate::jumppad::player_left(Id32::new(crate::territory::CHARACTER_TYPE, character));
        me.pos = *pos.lock().unwrap();
        match store.save(character, &me) {
            Ok(()) => info!(character, pos = ?me.pos, "character saved"),
            Err(e) => warn!(character, "saving character failed: {e:#}"),
        }
    }
    Ok(())
}

/// Collectibles, boomboxes and exploration areas where the player is that it hasn't found yet.
fn report_finds(me: &Character, at: [f32; 3], outbox: &mpsc::UnboundedSender<world::Out>) {
    let places = crate::discovery::near(me.playfield, at).map(|f| (f.playfield, f.name.as_str()));
    let explored = crate::encounter::explored(me.playfield, at).into_iter().map(|n| (me.playfield, n));
    for (playfield, name) in places.chain(explored) {
        if !me.finds.contains_key(&format!("{playfield}:{name}")) {
            let found = crate::achievements::Event::Found { playfield, name: name.to_string() };
            let _ = outbox.send(world::Out::Progress(found));
        }
    }
}

/// A minifigure's upgrade item stats changed (`stats`, its active upgrades were `old`): sends
/// them, and for a team minifigure applies the upgrades' effects anew; the active one's health
/// follows ("Fit", "Tough").
#[allow(clippy::too_many_arguments)]
async fn send_upgrades(conn: &mut Conn, character: u32, me: &Character, collection: &Collection, template: u32, old: u32, stats: &[(u32, u32)]) -> anyhow::Result<()> {
    let active = me.upgrade(template);
    let (inv, pos) = match me.team.iter().find(|&&(_, t)| t == template) {
        Some(&(slot, _)) => (crate::collection::TEAM_INVENTORY, slot),
        None => (crate::collection::COLLECTION_INVENTORY, collection.position_of(template).unwrap_or(0)),
    };
    for &(stat, value) in stats {
        // SetItemStat(const ID32& inventory, int pos, int stat, int value)
        let args = Id32::new(inv, character).write(Writer::new()).i32(pos as i32).i32(stat as i32).i32(value as i32).finish();
        conn.send_as(Some(character), Endpoint::GameCharInterface, game_char_interface::SET_ITEM_STAT, args).await?;
    }
    if inv == crate::collection::TEAM_INVENTORY {
        // ApplyMinifigureUpgradesOnClient(uint32 minifigure, int gameActionLists, int
        // activatedUpgrades): remove the old upgrades' effects (3), apply the new (1)
        for (lists, mask) in [(3, old), (1, active)] {
            let args = Writer::new().u32(template).i32(lists).i32(mask as i32).finish();
            conn.send_as(Some(character), Endpoint::GameCharInterfaceProject, game_char_interface_project::APPLY_MINIFIGURE_UPGRADES_ON_CLIENT, args)
                .await?;
        }
    }
    if me.team.first().is_some_and(|&(_, t)| t == template) {
        let id = Id32::new(crate::territory::CHARACTER_TYPE, character);
        let blob = iir::pass_blob(&health_records(id, template, active, me.level()));
        conn.send_as(Some(character), Endpoint::GameserverToClientRPC, client::PASS_BLOB, blob).await?;
    }
    Ok(())
}

/// A new character named `name`, starting in `playfield`.
pub fn new_character(name: String, playfield: u32) -> Character {
    Character {
        name,
        playfield,
        pos: START_POS,
        team: START_TEAM.to_vec(),
        stars: 0,
        upgrades: Default::default(),
        tokens: [(TOKEN_HEALTH_POTIONS, (START_POTIONS, 0))].into(),
        xp: 0,
        level: 0,
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
    }
}

/// Characters are saved on disconnect and, while moving, at most this often.
const SAVE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10);

/// The facing in a `CharDCMoveIIR_t` (u8 flags, position, then a yaw-only quaternion (qy, qw)
/// if flags & 0x10, else (qx, qy, qz, qw)), as a forward direction (x, z): yaw = 2·atan2(qy, qw),
/// forward = (sin yaw, cos yaw). The client turns the character towards the mouse on a cast and
/// reports it here.
fn reported_facing(rest: &[u8]) -> Option<[f32; 2]> {
    let f = |i: usize| rest.get(i..i + 4).map(|b| f32::from_be_bytes(b.try_into().unwrap()));
    let (qy, qw) = if rest.first()? & 0x10 != 0 { (f(13)?, f(17)?) } else { (f(17)?, f(25)?) };
    let (x, z) = (2.0 * qy * qw, qw * qw - qy * qy);
    let len = (x * x + z * z).sqrt();
    (len > 0.01).then(|| [x / len, z / len])
}

/// How a movement IIR says the character moves: keyboard movement (`CharDCMoveIIR_t`: u8 flags,
/// position, rotation (2 floats if flags & 0x10, else 4), u8 alignment, then the velocity unless
/// flags & 0x04), or a click-to-move path (`MoveTaskIIR_c`: from the start along the waypoints, at
/// the players' run speed, as `PLAYER_SPEEDS` makes the client walk it).
/// How often the portals are checked between movement reports.
const PORTAL_TICK: std::time::Duration = std::time::Duration::from_millis(250);

/// Portals: the client sends nothing when the player steps into one, so the server opens the
/// portal's teleport window then (once per visit, live's server scripts did this), or asks
/// Yes/No for a portal whose window would be empty, and closes it when they step out again.
/// Not right after arriving (login, teleport, maybe next to one): portals open once the player
/// has been clear of them. Using a portal's door (click or E) opens it any time; that window
/// stays until the player walks well clear of the portals.
#[derive(Default)]
struct Portals {
    /// the destination whose window is open
    shown: Option<u32>,
    /// whether it was opened by using the portal's door
    used: bool,
    /// the open message box (plain windows): its cookie and where it leads
    prompt: Option<(u64, u32)>,
    cookies: u64,
    armed: bool,
    playfield: Pf,
}

impl Portals {
    fn next_cookie(&mut self) -> u64 {
        self.cookies += 1;
        self.cookies
    }

    /// Opens teleport window `window` as a portal would (one that would be empty asks Yes/No).
    async fn open_window(&mut self, conn: &mut Conn, character: u32, window: u32) -> anyhow::Result<()> {
        if let Some(to) = crate::teleport::plain_window_destination(window) {
            let cookie = self.next_cookie();
            self.prompt = Some((cookie, to));
            let args = message_box_args(&crate::teleport::travel_question(to), cookie);
            conn.send_as(Some(character), Endpoint::GameCharInterface, game_char_interface::SCRY_SHOW_MESSAGE_BOX, args).await?;
            return Ok(());
        }
        // (an open one would stay instead)
        conn.send_as(Some(character), Endpoint::GameCharInterfaceProject, game_char_interface_project::SCRY_CLOSE_TELEPORT_MESSAGE_BOX, Bytes::new()).await?;
        let args = Writer::new().u32(window).i32(0).finish();
        conn.send_as(Some(character), Endpoint::GameCharInterfaceProject, game_char_interface_project::SCRY_SHOW_TELEPORT_MESSAGE_BOX, args).await?;
        Ok(())
    }

    /// The player is at `p` on `playfield`.
    async fn check(&mut self, conn: &mut Conn, character: u32, playfield: Pf, p: [f32; 3]) -> anyhow::Result<()> {
        let arrived = crate::teleport::take_arrival(character);
        if playfield != self.playfield || arrived {
            self.armed = false;
            self.playfield = playfield;
            info!(?p, "portals wait until the player is clear of them");
        }
        if !self.armed && crate::teleport::clear_of_portals(playfield, p) {
            self.armed = true;
            info!(?p, "portals open again");
        }
        match crate::teleport::portal_at(playfield, p).filter(|_| self.armed) {
            Some((to, window)) if self.shown != Some(to) => {
                info!(to, window, "portal");
                self.used = false;
                self.show(conn, character, to, window).await?;
            }
            Some(_) => {}
            None if self.shown.is_some() && (!self.used || crate::teleport::clear_of_portals(playfield, p)) => {
                self.shown = None;
                self.close(conn, character).await?;
            }
            None => {}
        }
        Ok(())
    }

    /// The player used the door of the portal to `to` with `window` (click or E).
    async fn use_door(&mut self, conn: &mut Conn, character: u32, (to, window): (u32, u32)) -> anyhow::Result<()> {
        info!(to, window, "portal door used");
        self.used = true;
        self.close(conn, character).await?;
        self.show(conn, character, to, window).await
    }

    /// Opens the window of the portal to `to`: `window`, or Yes/No for a window that would be
    /// empty.
    async fn show(&mut self, conn: &mut Conn, character: u32, to: u32, window: u32) -> anyhow::Result<()> {
        self.shown = Some(to);
        if crate::teleport::plain_window(window) {
            let cookie = self.next_cookie();
            self.prompt = Some((cookie, to));
            let args = message_box_args(&crate::teleport::travel_question(to), cookie);
            conn.send_as(Some(character), Endpoint::GameCharInterface, game_char_interface::SCRY_SHOW_MESSAGE_BOX, args).await?;
        } else {
            let args = Writer::new().u32(window).i32(0).finish();
            conn.send_as(Some(character), Endpoint::GameCharInterfaceProject, game_char_interface_project::SCRY_SHOW_TELEPORT_MESSAGE_BOX, args).await?;
        }
        Ok(())
    }

    /// Closes the open portal window or Yes/No box.
    async fn close(&mut self, conn: &mut Conn, character: u32) -> anyhow::Result<()> {
        if let Some((cookie, _)) = self.prompt.take() {
            let args = Writer::new().u64(cookie).finish();
            conn.send_as(Some(character), Endpoint::GameCharInterface, game_char_interface::SCRY_CLOSE_MESSAGE_BOX, args).await?;
        } else {
            conn.send_as(Some(character), Endpoint::GameCharInterfaceProject, game_char_interface_project::SCRY_CLOSE_TELEPORT_MESSAGE_BOX, Bytes::new())
                .await?;
        }
        Ok(())
    }
}

fn reported_motion(kind: &str, rest: &[u8]) -> Option<world::Motion> {
    let f = |i: usize| rest.get(i..i + 4).map(|b| f32::from_be_bytes(b.try_into().unwrap()));
    let at = |i: usize| Some([f(i)?, f(i + 4)?, f(i + 8)?]);
    let since = std::time::Instant::now();
    if kind == "CharDCMoveIIR_t" {
        let flags = *rest.first()?;
        let vel = if flags & 0x04 != 0 { [0.0; 3] } else { at(if flags & 0x10 != 0 { 22 } else { 30 })? };
        return Some(world::Motion::Steer { at: at(1)?, vel, since });
    }
    let n = rest.get(28..32).map(|b| u32::from_be_bytes(b.try_into().unwrap()))? as usize;
    if n == 0 || n > 64 {
        return None;
    }
    let points = std::iter::once(at(1)).chain((0..n).map(|i| at(32 + 12 * i))).collect::<Option<Vec<_>>>()?;
    let speed = PLAYER_SPEEDS.iter().find(|(stat, _)| *stat == STAT_RUN_SPEED).map_or(5.6, |&(_, v)| v);
    Some(world::Motion::Path { points, speed, since })
}

/// Where a movement IIR says the character is (CharDCMoveIIR_t) or is heading
/// (MoveTaskIIR_c: the path's last waypoint). See `move_record` / `move_task_record`.
fn reported_position(kind: &str, rest: &[u8]) -> Option<[f32; 3]> {
    let f = |i: usize| rest.get(i..i + 4).map(|b| f32::from_be_bytes(b.try_into().unwrap()));
    let at = |i: usize| Some([f(i)?, f(i + 4)?, f(i + 8)?]);
    match kind {
        "CharDCMoveIIR_t" => at(1),
        _ => {
            let n = rest.get(28..32).map(|b| u32::from_be_bytes(b.try_into().unwrap()))? as usize;
            if n == 0 || n > 64 {
                return None;
            }
            at(32 + 12 * (n - 1))
        }
    }
}

/// The IIRs that make a character exist on a client: its DreamWorld GameObject, the dynel,
/// and its stats. The player's own client additionally needs `full_player_character` (and
/// sends its own CharInPlayIIR_t); others get `remote_spawn_records`.
pub fn spawn_records(id: Id32, c: &CharSpec) -> Vec<Bytes> {
    vec![
        create_game_object(id, c.playfield, c.pos),
        simple_char_full_update(id, c),
        int32_stat(id, STAT_LIFE, c.max_health),
        int32_stat(id, STAT_HEALTH, c.health),
        int32_stat(id, STAT_LEVEL, c.level),
        // collision capsule radius = CharRadius x MonsterScale / 10000 (SimpleChar_t::SetStat)
        int32_stat(id, STAT_CHAR_RADIUS, 50),
        int32_stat(id, STAT_COMMAND_QUEUE_SIZE, 3),
        // unlocked team slots: below 3 the client refuses moves into slots 8/20
        // (ItemHolder::IsLegalItemPlacement)
        int32_stat(id, STAT_MAX_EQUIPPED_WEAPONS, 3),
        // the client computes Life itself: level × HealthPerLevel + the minifigure's life
        // (SimpleChar_t::CalculateLife); the original server sent 100
        int32_stat(id, STAT_HEALTH_PER_LEVEL, 100),
    ]
}

/// What other clients need to see a character: its spawn records, already in play.
pub fn remote_spawn_records(id: Id32, c: &CharSpec) -> Vec<Bytes> {
    let mut records = spawn_records(id, c);
    records.push(iir::record("CharInPlayIIR_t", id, false, |w| w));
    records
}

/// The player's playfield and character, as at login: PlayfieldFIIR_t, the character's
/// GameObject and dynel, FullPlayerCharacterIIR, stats; then the token counts
/// (UpdateTokenAmount(uint32 token, uint64 gained, uint64 spent): potions 150, stars 100).
async fn send_world(
    conn: &mut Conn,
    character: u32,
    me: &Character,
    place: Pf,
    collection: &Collection,
) -> anyhow::Result<()> {
    let id = Id32::new(crate::territory::CHARACTER_TYPE, character);
    let mut records = vec![playfield_full_update(place)];
    records.extend(spawn_records(id, &me.spec(place)));
    records.insert(3, full_player_character(id, me, collection));
    let gm_level = crate::sessions::gm_level(character);
    if gm_level != 0 {
        records.push(int32_stat(id, STAT_GM_LEVEL, gm_level));
    }
    records.extend(xp_records(id, me));
    // the player's achievement counters (kills, smashes): the character is new to the client
    records.extend(crate::achievements::player_counters(me).into_iter().map(|(stat, value)| stat_record(id, stat, value)));
    // every player is a member (member chests, the member smash timer)
    records.push(int32_stat(id, crate::smash::STAT_SUBSCRIPTION_FLAGS, crate::smash::MEMBER_FLAGS));
    conn.send_as(Some(character), Endpoint::GameserverToClientRPC, client::PASS_BLOB, iir::pass_blob(&records)).await?;
    // the smashed minifigures' locks (the character's items were sent anew)
    for (&template, &until) in &me.smashed {
        conn.send_as(Some(character), Endpoint::GameCharInterface, game_char_interface::LOCK_TEMPLATE, crate::smash::lock_args(template, until))
            .await?;
    }
    let saved = me.tokens.iter().map(|(&t, &(g, s))| (t, g, s));
    for (token, gained, spent) in [(TOKEN_STARS, me.stars, 0)].into_iter().chain(saved) {
        send_token(conn, character, token, gained, spent).await?;
    }
    Ok(())
}

/// Moves the player to `playfield` (arriving at its destination point, see teleport.rs):
/// IntendedTeleportIIR_c + n3TeleportIIR_t, leave the old playfield's world, then the new
/// playfield as at login. The client answers with CharInPlayIIR_t, which joins the new world.
#[allow(clippy::too_many_arguments)]
async fn teleport(
    conn: &mut Conn,
    store: &Store,
    character: u32,
    me: &mut Character,
    collection: &Collection,
    presence: &mut Option<world::Presence>,
    pos: &world::Position,
    playfield: Pf,
    place: &mut Pf,
    spot: Option<u32>,
    at: Option<([f32; 3], f32)>,
) -> anyhow::Result<()> {
    // arrive where asked (test commands), at the chosen teleport spot (World Map); through a
    // teleport window, by the portal back to where the player came from; else at the
    // playfield's arrival point
    let model = playfield.model();
    let arrival = at
        .or_else(|| match spot {
            Some(g) => crate::teleport::spot(model, g).map(|s| (s.pos, s.rot)),
            None => crate::teleport::return_point(model, me.playfield).map(|d| (d.pos, d.rot)),
        })
        .or_else(|| crate::teleport::destination(model).map(|d| (d.pos, d.rot)));
    let Some((arrive_pos, arrive_rot)) = arrival else {
        warn!(?playfield, "no destination data for playfield");
        return Ok(());
    };
    info!(from = ?*place, to = ?playfield, ?spot, "teleport");
    let id = Id32::new(crate::territory::CHARACTER_TYPE, character);
    // a dialogue going on ends here (its window and cinematic would stay up in the new world)
    let mut records = crate::knubot::end(id);
    records.extend([
        crate::teleport::intended_teleport_record(id, playfield),
        crate::teleport::teleport_record(id, playfield, arrive_pos, arrive_rot),
    ]);
    conn.send_as(Some(character), Endpoint::GameserverToClientRPC, client::PASS_BLOB, iir::pass_blob(&records)).await?;
    // leaving: despawns the character for everyone in the old playfield
    *presence = None;
    *place = playfield;
    me.playfield = model;
    me.pos = arrive_pos;
    *pos.lock().unwrap() = arrive_pos;
    crate::teleport::arrived(character);
    // the new world's HUD shows the first team position as the active portrait, as at login
    active_at_first_position(&mut me.team);
    if let Err(e) = store.save(character, me) {
        warn!(character, "saving character failed: {e:#}");
    }
    // let the client tear down the old playfield first
    tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
    send_world(conn, character, me, *place, collection).await
}


/// The XP bar's stats: XP, LastXP and NextXP of the level (XP 0 without progression).
fn xp_records(id: Id32, me: &Character) -> Vec<Bytes> {
    let level = me.level();
    let (last, next) = if crate::progression::enabled() {
        (crate::progression::last_xp(level), crate::progression::next_xp(level))
    } else {
        (0, 0)
    };
    [(STAT_XP, me.xp), (STAT_LAST_XP, last), (STAT_NEXT_XP, next)]
        .into_iter()
        .map(|(stat, v)| int32_stat(id, stat, v.min(i32::MAX as u64) as u32))
        .collect()
}

/// Adds XP from a defeated enemy (progression), levelling up as far as the area's cap allows.
/// A character at or above the cap keeps its level and gains no XP there (the user's account of
/// live: the cap only stops XP).
fn gain_xp(me: &mut Character, character: u32, xp: u64) -> Vec<Bytes> {
    let id = Id32::new(crate::territory::CHARACTER_TYPE, character);
    let before = me.level();
    if before >= crate::progression::effective_cap(me.playfield) {
        return Vec::new();
    }
    me.xp += xp;
    let level = crate::progression::level_for(me.xp, crate::progression::effective_cap(me.playfield)).max(before);
    let mut records = Vec::new();
    if level != before {
        me.level = level;
        info!(character, level, xp = me.xp, "level up");
        records.push(int32_stat(id, STAT_LEVEL, me.level()));
        // the client recomputes Life from the level; send it and the health with it
        if let Some(&(_, t)) = me.team.first() {
            records.extend(health_records(id, t, me.upgrade(t), me.level()));
        }
    }
    records.push(stat_record(id, STAT_XP, me.xp.min(i32::MAX as u64) as u32));
    if level != before {
        records.extend(xp_records(id, me).into_iter().skip(1));
    }
    records
}

/// Runs the achievement rules for `event` (or the login when `None`) on the character.
/// `None`: the login (tags and counters again); `Some((None, _))`: a re-check.
fn progress(
    me: &mut Character,
    collection: &Collection,
    character: u32,
    event: Option<(Option<&crate::achievements::Event>, u64)>,
) -> crate::achievements::Output {
    let team = me.team.clone();
    let slots = collection.slots.clone();
    let bought: HashMap<u32, u32> = crate::collection::all_minifigures().into_iter().map(|t| (t, me.purchased(t).count_ones())).collect();
    let bought = move |t: u32| bought.get(&t).copied().unwrap_or(0);
    let locate = move |t: u32| match team.iter().find(|&&(_, x)| x == t) {
        Some(&(slot, _)) => (crate::collection::TEAM_INVENTORY, slot),
        None => (crate::collection::COLLECTION_INVENTORY, slots.iter().position(|s| *s == Some(t)).unwrap_or(0) as u32),
    };
    let owned_set = me.collection.clone();
    let owned = move |t: u32| owned_set.as_ref().is_none_or(|o| o.contains_key(&t));
    let ctx = crate::achievements::Ctx {
        character,
        active: me.team.first().map(|&(_, t)| t),
        locate: &locate,
        upgrades: &bought,
        owned: &owned,
    };
    match event {
        Some((Some(e), times)) => crate::achievements::on_event(me, &ctx, e, times),
        Some((None, _)) => crate::achievements::check(me, &ctx),
        None => crate::achievements::login(me, &ctx),
    }
}

async fn send_progress(
    conn: &mut Conn,
    character: u32,
    place: Pf,
    me: &mut Character,
    collection: &mut Collection,
    out: crate::achievements::Output,
) -> anyhow::Result<()> {
    if !out.records.is_empty() {
        conn.send_as(Some(character), Endpoint::GameserverToClientRPC, client::PASS_BLOB, iir::pass_blob(&out.records)).await?;
    }
    for (endpoint, method, args) in out.rpcs {
        conn.send_as(Some(character), endpoint, method, args).await?;
    }
    if out.stars > 0 {
        send_token(conn, character, TOKEN_STARS, me.stars, 0).await?;
    }
    // items the world editor's drop rules drop (where the defeated one was, else at the player)
    for (dropper, item) in out.drops {
        let player = Id32::new(crate::territory::CHARACTER_TYPE, character);
        let at = world::whereabouts(dropper).map(|(_, p)| p).or_else(|| world::live_position(player)).unwrap_or(me.pos);
        crate::loot::drop_items(player, place, at, dropper, vec![item]);
    }
    // new achievements may change the aura (the reward scripts recast the aura applier)
    if let Some(&(_, template)) = me.team.first() {
        crate::auras::apply(Id32::new(crate::territory::CHARACTER_TYPE, character), template, &me.tags, false);
    }
    // minifigures given for an achievement (Chicken Suit Guy)
    for (tag, template) in crate::loot::achievement_minifigures() {
        if me.tags.contains_key(&tag) {
            grant_minifigure(conn, character, me, collection, template).await?;
        }
    }
    Ok(())
}

/// Adds `template` to a limited collection (a bag, a minifigure built from parts, an
/// achievement): AddItem into the Collection inventory, which unlocks it in the client's
/// minifigure list. False if the character owns it already (or owns every minifigure).
async fn grant_minifigure(conn: &mut Conn, character: u32, me: &mut Character, collection: &mut Collection, template: u32) -> anyhow::Result<bool> {
    let Some(owned) = me.collection.as_mut() else { return Ok(false) };
    if owned.contains_key(&template) {
        return Ok(false);
    }
    owned.insert(template, crate::smash::now_ms() / 1000);
    let pos = collection.free_position();
    collection.put(pos, template);
    let item = crate::collection::new_item(template, &me.item_state(template));
    let args = Id32::new(crate::collection::COLLECTION_INVENTORY, character).write(Writer::new()).i32(pos as i32).proto(&item).u8(0).finish();
    conn.send_as(Some(character), Endpoint::GameCharInterface, game_char_interface::ADD_ITEM, args).await?;
    info!(character, template, pos, "minifigure obtained");
    Ok(true)
}

/// Sends IIR records to the player (nothing when there are none).
async fn send_records(conn: &mut Conn, character: u32, records: Vec<Bytes>) -> anyhow::Result<()> {
    if !records.is_empty() {
        conn.send_as(Some(character), Endpoint::GameserverToClientRPC, client::PASS_BLOB, iir::pass_blob(&records)).await?;
    }
    Ok(())
}

/// A charge ability being held (see projectile.rs `charge_items`).
struct Charge {
    spell: u32,
    started: std::time::Instant,
    /// Cleared on release: stops re-applying the channel buff.
    active: Arc<std::sync::atomic::AtomicBool>,
    target: Option<Id32>,
    damage: u32,
    /// The running copy it was started in.
    place: Pf,
}

/// How long the others' copy of a cast is remembered for a release or abort.
const OBSERVED_CAST_KEEP: std::time::Duration = std::time::Duration::from_secs(60);

/// Ends the others' copy of a cast (server command id, target, spell, when).
fn end_observed(caster: Id32, observed: Option<(u32, Id32, u32, std::time::Instant)>) {
    if let Some((id, target, spell, _)) = observed {
        world::send_playfield(caster, false, world::Out::Records(crate::npc::observed_end(caster, target, spell, id)));
    }
}

/// A held charge, from press to release: shows its channel buff (the growing orb) on the caster,
/// re-cast every channel tick, and if the player keeps holding past full charge plus
/// CHARGE_HOLD, fires at full charge and ends the client's channel itself (BCC action 20 back to
/// the caster). Released earlier, the buff is removed and `fire_charge` fires.
#[allow(clippy::too_many_arguments)]
fn run_charge(
    caster: Id32,
    playfield: Pf,
    spell: u32,
    active: Arc<std::sync::atomic::AtomicBool>,
    pos: world::Position,
    facing: Arc<Mutex<[f32; 2]>>,
    target: Option<Id32>,
    full_damage: u32,
    end_record: Bytes,
    auto_fired: Arc<Mutex<Option<(u32, std::time::Instant)>>>,
) {
    let Some(p) = crate::projectile::params(spell) else { return };
    let full = std::time::Duration::from_secs_f32(p.channel_interval * p.charge_items.len().saturating_sub(1) as f32);
    let until = full + CHARGE_HOLD;
    let buff = p.channel_buff;
    let counter_buff = p.counter_buff;
    let full_item = p.charge_items.last().copied();
    let tick = std::time::Duration::from_secs_f32(p.channel_interval.max(0.1));
    tokio::spawn(async move {
        // The channel buff (Big Blast: 635802, the orb, TimeExist 0.75 s) is cast on the press
        // and again on every channel tick (the spell's tick action list), as live did; each
        // add lasts 1.5 ticks so the orb stays up between ticks.
        let duration_cs = (tick.as_secs_f32() * 150.0) as i32;
        let started = std::time::Instant::now();
        let mut next_tick = std::time::Duration::ZERO;
        // channel ticks so far (the press isn't one): the counter buff's count, the charge level
        let mut ticks = 0u32;
        while active.load(std::sync::atomic::Ordering::Relaxed) && started.elapsed() < until {
            if started.elapsed() >= next_tick {
                let mut records = Vec::new();
                if let Some(buff) = buff {
                    records.push(crate::effects::buff_add(caster, buff, caster, duration_cs));
                }
                // each tick's AddBuffCounter: the first adds the buff with count 1, later ones
                // update the count; it lasts until the charge window ends
                if let Some(counter) = counter_buff
                    && next_tick > std::time::Duration::ZERO
                {
                    ticks += 1;
                    let left = (until.saturating_sub(started.elapsed()).as_secs_f32() * 100.0) as u32;
                    records.push(if ticks == 1 {
                        crate::effects::buff_add_counter(caster, counter, caster, 1, left as i32)
                    } else {
                        crate::effects::buff_update_counter(caster, counter, caster, ticks, left)
                    });
                }
                if !records.is_empty() {
                    world::send_playfield(caster, true, world::Out::Records(records));
                }
                next_tick += tick;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        if active.swap(false, std::sync::atomic::Ordering::Relaxed) {
            // held too long: fire at full charge, in the direction faced now
            let here = *pos.lock().unwrap();
            let f = *facing.lock().unwrap();
            let towards = target.and_then(|t| crate::projectile::aim_for(spell, t, here)).unwrap_or([here[0] + f[0], here[1], here[2] + f[1]]);
            info!(spell, "charge fired automatically");
            crate::projectile::launch(caster, playfield, spell, full_item, here, towards, target, full_damage);
            *auto_fired.lock().unwrap() = Some((spell, std::time::Instant::now()));
            world::send_to(caster, world::Out::Records(vec![end_record]));
        }
        // the end list removes the counter buff, then the channel buff
        let removes: Vec<Bytes> = [counter_buff.filter(|_| ticks > 0), buff]
            .into_iter()
            .flatten()
            .map(|b| crate::effects::buff_remove(caster, b, caster))
            .collect();
        if !removes.is_empty() {
            world::send_playfield(caster, true, world::Out::Records(removes));
        }
    });
}

/// Fires a released charge: the projectile for the charge level reached.
fn fire_charge(c: &Charge, me: &Character, caster: Id32, here: [f32; 3], heading: [f32; 2]) {
    if !c.active.swap(false, std::sync::atomic::Ordering::Relaxed) {
        return; // already fired automatically after holding too long
    }
    let Some(p) = crate::projectile::params(c.spell) else { return };
    let held = c.started.elapsed().as_secs_f32();
    let item = p.charged_item(held);
    // the charge level picks the hit's AbilityMod (Big Blast: 0.9 .. 2.5)
    let level = (held / p.channel_interval.max(0.05)) as usize;
    let damage = me.team.first().map_or(c.damage, |&(_, t)| crate::combat::damage(t, me.upgrade(t), c.spell, Some(level), me.level()));
    // The release resends the press-time command, so its point is where the player aimed when
    // they started charging; aim with the facing they have now instead (the client keeps
    // reporting it while they turn). A targeted cast still goes to its target.
    let towards = c.target.and_then(|t| crate::projectile::aim_for(c.spell, t, here)).unwrap_or([here[0] + heading[0], here[1], here[2] + heading[1]]);
    info!(spell = c.spell, held, item, "charge released");
    crate::projectile::launch(caster, c.place, c.spell, Some(item), here, towards, c.target, damage);
}

/// Fires charges whose release never arrived.
fn fire_stale_charges(charges: &mut HashMap<u32, Charge>, me: &Character, caster: Id32) {
    let stale: Vec<u32> = charges.iter().filter(|(_, c)| c.started.elapsed() > CHARGE_TIMEOUT).map(|(k, _)| *k).collect();
    for k in stale {
        if let Some(c) = charges.remove(&k) {
            let here = me.pos;
            fire_charge(&c, me, caster, here, [0.0, 1.0]);
        }
    }
}

/// What changes when `template` becomes the active minifigure: its costume (MonsterData, also
/// shown to everyone else) and its own health.
fn active_minifigure_records(id: Id32, template: u32, upgrades: u32, level: u32, presence: &Option<world::Presence>) -> Vec<Bytes> {
    set_costume(id, monster_data_for(template));
    let costume = int32_stat(id, STAT_MONSTER_DATA, monster_data_for(template));
    if let Some(p) = presence {
        world::broadcast(p.id(), vec![costume.clone()]);
    }
    info!(template, "active minifigure");
    let mut records = vec![costume];
    records.extend(health_records(id, template, upgrades, level));
    records
}

/// Players' current look (MonsterData), for `refresh_costumes`.
static COSTUMES: std::sync::LazyLock<Mutex<HashMap<Id32, u32>>> = std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

fn set_costume(id: Id32, monster_data: u32) {
    COSTUMES.lock().unwrap().insert(id, monster_data);
}

/// A player character built from its spawn records (full update with its look) shows the look
/// but doesn't play its movement animations (it slides in one pose) until the look changes, as
/// a minifigure switch does (SimpleChar_t::SetStat MonsterData -> SetAnimatedMesh, which
/// ignores an unchanged mesh). So, a moment after `joined` entered a playfield, its look and
/// the other players' looks there change to the generic body and back: for `joined` on every
/// client, and for the others on `joined`'s client.
fn refresh_costumes(joined: Id32) {
    tokio::spawn(async move {
        tokio::time::sleep(COSTUME_REFRESH_DELAY).await;
        let flip = |id: Id32, look: u32| vec![int32_stat(id, STAT_MONSTER_DATA, GENERIC_LOOK), int32_stat(id, STAT_MONSTER_DATA, look)];
        let costumes = COSTUMES.lock().unwrap().clone();
        if let Some(&look) = costumes.get(&joined) {
            world::send_playfield(joined, true, world::Out::Records(flip(joined, look)));
        }
        let others: Vec<Bytes> = world::neighbours(joined)
            .into_iter()
            .filter_map(|other| costumes.get(&other).map(|&look| flip(other, look)))
            .flatten()
            .collect();
        if !others.is_empty() {
            world::send_to(joined, world::Out::Records(others));
        }
        // the flips rebuilt the characters: show the auras on them again
        let mine = crate::auras::records(joined);
        if !mine.is_empty() {
            world::send_playfield(joined, true, world::Out::Records(mine));
        }
        let theirs: Vec<Bytes> = world::neighbours(joined).into_iter().flat_map(crate::auras::records).collect();
        if !theirs.is_empty() {
            world::send_to(joined, world::Out::Records(theirs));
        }
    });
}

/// The generic body ("Aztec_Warrior", the minifigure templates' own MonsterData stat).
const GENERIC_LOOK: u32 = 2212;
/// After the spawn records, so the first build of the character is done (assumed).
const COSTUME_REFRESH_DELAY: std::time::Duration = std::time::Duration::from_millis(1500);


/// How long `!test` enemies stay.
const TEST_ENEMY_LIFETIME: std::time::Duration = std::time::Duration::from_secs(15 * 60);

/// Makes `templates` the team: minifigures already on it keep their slots (and the active one
/// stays active), the others take the freed slots in order (the one replacing the active
/// minifigure first, and it becomes active); minifigures leave and join the collection as an
/// equip would move them. `team[0]` is the active one. The client gets the team with the
/// character's next full update (a teleport).
fn set_team(me: &mut Character, collection: &mut Collection, templates: &[u32]) {
    let active = me.team.first().copied();
    let kept: Vec<(u32, u32)> = me.team.iter().copied().filter(|(_, t)| templates.contains(t)).collect();
    // free slots: the active one's first (when it leaves), then the rest in order
    let mut free: Vec<u32> = me.team.iter().map(|&(s, _)| s).chain(START_TEAM.iter().map(|&(s, _)| s)).collect();
    free.sort_unstable();
    free.dedup();
    free.retain(|s| !kept.iter().any(|(k, _)| k == s));
    if let Some(i) = active.and_then(|(a, _)| free.iter().position(|&s| s == a)) {
        let a = free.remove(i);
        free.insert(0, a);
    }
    let joining: Vec<u32> = templates.iter().copied().filter(|t| !kept.iter().any(|(_, k)| k == t)).collect();
    let mut freed = Vec::new();
    for &t in &joining {
        if let Some(p) = collection.position_of(t) {
            collection.take(p);
            freed.push(p);
        }
    }
    for &(_, old) in &me.team {
        if !templates.contains(&old) {
            let p = if freed.is_empty() { collection.slots.len() as u32 } else { freed.remove(0) };
            collection.put(p, old);
        }
    }
    let mut team: Vec<(u32, u32)> = kept.into_iter().chain(free.into_iter().zip(joining)).collect();
    team.sort_unstable_by_key(|&(s, _)| s);
    // the active minifigure stays active if it is still on the team; else the test's first one
    let active_template = active.map(|(_, t)| t).filter(|t| templates.contains(t)).or(templates.first().copied());
    if let Some(i) = team.iter().position(|&(_, t)| Some(t) == active_template) {
        let entry = team.remove(i);
        team.insert(0, entry);
    }
    me.team = team;
}

/// Moves the active minifigure (`team[0]`) to the team's first inventory position, swapping
/// with the one there. Live logged in with ActiveWeapon 6 every time; with the active one
/// elsewhere the client used its spells but its HUD showed the minifigure at position 6 as
/// the active portrait (also after a teleport, which sends the world again).
fn active_at_first_position(team: &mut [(u32, u32)]) {
    let Some(first) = team.iter().map(|&(p, _)| p).min() else { return };
    if let Some(j) = team.iter().position(|&(p, _)| p == first)
        && j != 0
    {
        team[j].0 = team[0].0;
        team[0].0 = first;
    }
}

/// Sends LearnSpell for everything the team's minifigures can cast.
async fn learn_team_spells(conn: &mut Conn, character: u32, me: &Character) -> anyhow::Result<()> {
    for spell in me.team.iter().flat_map(|&(_, t)| crate::collection::spells_to_learn(t)) {
        let args = Writer::new().i32(spell as i32).finish();
        conn.send_as(Some(character), Endpoint::GameCharInterface, game_char_interface::LEARN_SPELL, args).await?;
    }
    Ok(())
}

/// EquipItemCommand data: ID32 source inventory, u32 source position, ID32 destination
/// inventory, u32 destination slot, ID32 swap inventory, u32 swap position (where the
/// displaced item goes), ... Applies a collection → team move to our model; returns the new
/// active minifigure if the active slot changed.
fn equip(me: &mut Character, collection: &mut Collection, data: &[u8]) -> Option<(u32, u32)> {
    let u = |i: usize| data.get(i..i + 4).map(|b| u32::from_be_bytes(b.try_into().unwrap()));
    let (src_inv, src_pos, dst_inv, dst_slot, swap_inv, swap_pos) = (u(0)?, u(8)?, u(12)?, u(20)?, u(24)?, u(32)?);
    info!(src_inv, src_pos, dst_inv, dst_slot, swap_inv, swap_pos, "equip");
    if src_inv != crate::collection::COLLECTION_INVENTORY || dst_inv != crate::collection::TEAM_INVENTORY {
        return None;
    }
    let template = collection.take(src_pos)?;
    let displaced = match me.team.iter_mut().find(|(s, _)| *s == dst_slot) {
        Some(entry) => Some(std::mem::replace(&mut entry.1, template)),
        None => {
            me.team.push((dst_slot, template));
            None
        }
    };
    if let Some(old) = displaced {
        collection.put(if swap_inv == crate::collection::COLLECTION_INVENTORY { swap_pos } else { src_pos }, old);
    }
    Some((dst_slot, template))
}

/// `GameCharInterface::ScryShowMessageBox(String text, int text1, int text2, int text3, int tip1,
/// int tip2, int tip3, int ctx, String custom name, uint64 cookie)`: a dialog box with `text`.
/// Button texts -1 and -1 make Yes and No (ScryMessageBox::Init); the answer comes back as
/// ScryMessageBoxAnswer with the cookie.
fn message_box_args(text: &str, cookie: u64) -> Bytes {
    Writer::new().string(text).i32(-1).i32(-1).i32(0).i32(0).i32(0).i32(0).i32(0).string("").u64(cookie).finish()
}

/// ObjectInteraction data: ID32 object, u32 action (6 use, `INTERACTION_STOP_USE`).
fn interaction_object(data: &[u8]) -> Option<(Id32, u32)> {
    let u = |i: usize| data.get(i..i + 4).map(|b| u32::from_be_bytes(b.try_into().unwrap()));
    Some((Id32::new(u(0)?, u(4)?), u(8)?))
}

/// A queued command from a BehaviorControlCenterIIR_c body: u8 action, u32 CommandType,
/// u32 length + CommandMessage protobuf, then the command's own data.
fn parse_command(rest: &[u8]) -> Option<(lmoxide_net::proto::CommandMessage, &[u8])> {
    use prost::Message;
    let len = u32::from_be_bytes(rest.get(5..9)?.try_into().ok()?) as usize;
    let msg = lmoxide_net::proto::CommandMessage::decode(rest.get(9..9 + len)?).ok()?;
    Some((msg, &rest[9 + len..]))
}

/// Makes `template` the active minifigure and sends its health (each minifigure has its own;
/// Life is the maximum).
fn health_records(id: Id32, template: u32, upgrades: u32, level: u32) -> Vec<Bytes> {
    let max = crate::combat::max_life(template, upgrades, level);
    let health = crate::npc::set_active_minifigure(id, template, max, crate::combat::mitigation(template, upgrades, level));
    crate::procs::set_active(id, template, upgrades, level);
    vec![int32_stat(id, STAT_LIFE, max), int32_stat(id, STAT_HEALTH, health)]
}

/// `CombatLogIIR` (AttackUtils_c::ActivateCombatLog 0x175bc26): the floating damage number.
/// u32 count, entries of 5 u32 (type, spell, amount, attacker instance, target instance), the
/// attacker ID32, u32 command queue id (0 = show now). Type 2 has no chat line. Attacker and
/// target must be type-50000 characters.
pub fn combat_log_record(attacker: Id32, target: Id32, spell: u32, amount: u32) -> Bytes {
    iir::record("CombatLogIIR", attacker, false, |w| {
        let w = w.u32(1).u32(2).u32(spell).u32(amount).u32(attacker.instance).u32(target.instance);
        attacker.write(w).u32(0)
    })
}

/// `UpdateMessage` op 1 (Activate 0x6ef4e0): removes the GameObject with this Id.
pub fn despawn_record(id: Id32) -> Bytes {
    iir::record("UpdateMessage", id, false, |w| id.write(w.u8(1)))
}

/// `CharDCMoveIIR_t` (PlayerMoveUpdateData::Read, direct-control/keyboard movement): u8 flags
/// (bit 4: yaw-only rotation; bit 2: no velocity; bit 3: no second vector block), position,
/// rotation y and w, u8 alignment mode, velocity, f32 rotation speed, u8, u8. On another
/// client's character in direct-control mode (players' vehicles are, while they have no move
/// task) Activate places it (smoothly within 3 m) and keeps it moving at `vel`.
pub fn move_record(id: Id32, pos: [f32; 3], yaw: f32, vel: [f32; 3]) -> Bytes {
    iir::record("CharDCMoveIIR_t", id, false, |w| {
        let w = w.u8(0x18).f32(pos[0]).f32(pos[1]).f32(pos[2]).f32((yaw / 2.0).sin()).f32((yaw / 2.0).cos()).u8(0);
        w.f32(vel[0]).f32(vel[1]).f32(vel[2]).f32(0.0).u8(0).u8(1)
    })
}

/// `MoveTaskIIR_c` as the client sends it for click-to-move: u8 0, start position, u8 0x0f,
/// 10 zero bytes, f32 (0.07 in samples), u32 n, n waypoints, 16 zero bytes. Layout copied from
/// client traffic; field meanings beyond the positions are unknown.
pub fn move_task_record(id: Id32, from: [f32; 3], to: [f32; 3]) -> Bytes {
    iir::record("MoveTaskIIR_c", id, false, |w| {
        let w = w.u8(0).f32(from[0]).f32(from[1]).f32(from[2]).u8(0x0f).u32(0).u32(0).u16(0).f32(0.072).u32(2);
        let w = [from, to].iter().fold(w, |w, p| w.f32(p[0]).f32(p[1]).f32(p[2]));
        w.u32(0).u32(0).u32(0).u32(0)
    })
}

/// Above 16,000,000, so clients treat the bot as a player (n3Dynel::IsNPC is false: its
/// vehicle takes direct-control movement like another player's).
const BOT_INSTANCE: u32 = 16_000_000 + 0x0b07;
/// Sender instance of the test commands' chat answers.
pub const SERVER_INSTANCE: u32 = 0x5e7e;

/// A server-side character that circles whoever else is on the playfield, standing in for a
/// second player (`--bot`).
pub async fn run_bot(playfield: Pf) {
    let id = Id32::new(crate::territory::CHARACTER_TYPE, BOT_INSTANCE);
    let (outbox, mut inbox) = mpsc::unbounded_channel();
    let pos = Arc::new(Mutex::new(START_POS));
    let spawn_pos = pos.clone();
    let bot = Character { name: "Bot".into(), ..new_character(String::new(), playfield.model()) };
    let spawn = move || remote_spawn_records(id, &Character { pos: *spawn_pos.lock().unwrap(), ..bot.clone() }.spec(playfield));

    let _presence = world::join(id, playfield, pos.clone(), spawn, outbox);
    set_costume(id, monster_data_for(START_TEAM[0].1));
    tokio::spawn(async move { while inbox.recv().await.is_some() {} });
    // runs around them like a player with the keyboard: a CharDCMoveIIR_t with the velocity
    // every 0.2 s (the client's own update rate while steering)
    const RADIUS: f32 = 4.0;
    const SPEED: f32 = 5.6;
    let mut t = 0.0f32;
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(200));
    // development: LMOXIDE_BOT_CAST="spell:secs:template" casts `spell` every 10 s (standing
    // still, looking like minifigure `template`) and ends it after `secs`, as another player's
    // held charge looks and ends to the onlookers
    let cast: Option<(u32, f32, u32)> = std::env::var("LMOXIDE_BOT_CAST").ok().and_then(|v| {
        let mut p = v.split(':');
        Some((p.next()?.parse().ok()?, p.next()?.parse().ok()?, p.next().and_then(|t| t.parse().ok()).unwrap_or(0)))
    });
    if let Some((_, _, template)) = cast.filter(|c| c.2 != 0) {
        set_costume(id, monster_data_for(template));
        world::broadcast(id, vec![int32_stat(id, STAT_MONSTER_DATA, monster_data_for(template))]);
    }
    let mut ticks = 0u32;
    loop {
        tick.tick().await;
        ticks += 1;
        if cast.is_some() && (ticks % 50 >= 45 || ticks % 50 < 20) && ticks % 50 != 0 {
            // stands still around the casts
            if ticks % 50 == 45 {
                let at = *pos.lock().unwrap();
                world::broadcast(id, vec![move_record(id, at, 0.0, [0.0; 3])]);
            }
            continue;
        }
        if let Some((spell, hold, _)) = cast
            && ticks % 50 == 0
        {
            let (observed, records) = crate::npc::observed_cast(id, id, spell);
            world::broadcast(id, records.to_vec());
            info!(spell, observed, "bot cast");
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs_f32(hold)).await;
                info!(spell, observed, "bot cast ended");
                world::broadcast(id, crate::npc::observed_end(id, id, spell, observed));
            });
        }
        t += 0.2 * SPEED / RADIUS;
        let center = world::positions(playfield)
            .into_iter()
            .find(|(other, _)| *other != id && other.kind == crate::territory::CHARACTER_TYPE && !crate::npc::is_npc(*other))
            .map_or(START_POS, |(_, p)| p);
        let at = [center[0] + RADIUS * t.cos(), center[1], center[2] + RADIUS * t.sin()];
        let dir = [-t.sin(), t.cos()];
        *pos.lock().unwrap() = at;
        let vel = [dir[0] * SPEED, 0.0, dir[1] * SPEED];
        world::broadcast(id, vec![move_record(id, at, dir[0].atan2(dir[1]), vel)]);
    }
}

/// ID32 type of playfields (key of the client's PlayfieldFactory map, 0x96cee0).
pub const PLAYFIELD_TYPE: u32 = 51100;

/// `PlayfieldFIIR_t`: n3PlayfieldFullUpdateIIR_t::ReadSubClass (0x7f60f0) = u32, u32, u32,
/// PlayfieldProxyData protobuf, u32, blob, blob; then PlayfieldFIIR_t (0x9d9b10) adds 6 u32.
/// Activate (0x7f5670) creates the playfield from the PlayfieldProxyData model id; the second
/// phase (0x7f5d30) reads blob 2 as dynamic pathfinding data (0xbd3900: u32 count, entries) and
/// blob 1 as a u32 count of nested IIRs. Both must hold at least a count: reading a u32 from an
/// empty stream yields garbage, which made the client allocate until it ran out of memory.
fn playfield_full_update(playfield: Pf) -> bytes::Bytes {
    let id = Id32::new(PLAYFIELD_TYPE, playfield.model());
    let proxy = PlayfieldProxyData {
        dimension_id: Some(1),
        model_id: Some(ProtoId32 { r#type: Some(PLAYFIELD_TYPE), instance: Some(playfield.model()) }),
        // key of the client's playfield map ("Inserting playfield N in map"); dynels refer to it
        // via ProtoGameCodeDynelIIR.playfield_instance
        running_instance: Some(playfield.key()),
        ..Default::default()
    };
    iir::record("PlayfieldFIIR_t", id, false, |w| {
        let empty_list = 0u32.to_be_bytes();
        let w = w.u32(0).u32(0).u32(0).proto(&proxy).u32(0).blob(&empty_list).blob(&empty_list);
        // 6 u32 (PlayfieldFIIR_t::ReadSubClass, +0xd8..0xec); the 5th is the area's level cap
        // (recorded: 11 in Pirate World, 0 = none in the tutorial)
        let cap = if crate::progression::enabled() { crate::progression::level_cap(playfield.model()) } else { 0 };
        w.u32(0).u32(0).u32(0).u32(0).u32(cap).u32(0)
    })
}

/// Version the client's CoreFullPlayerCharacterIIR reader expects (0x1b81edc).
const FULL_CHARACTER_VERSION: u32 = 24;
/// Empty 1009-encoded container.
const EMPTY: u32 = 0x3f1;

/// `FullPlayerCharacterIIR` (the player's own character), minimal: every list empty but the
/// header's tags and inventories.
/// Header protobuf `ProtoFullCharacterIIR`, then CoreFullPlayerCharacterIIR::ReadSubClass
/// (0x602ed0) and FullPlayerCharacterIIR's additions (0xe3f9f0):
///
/// ```text
/// u32 version (24)
/// u32 n,   n x u32                        0x9cd8c0
/// cont<u32>                               0x5f8d80
/// u32 n,   n x element                    0x9cdf60
/// cont<u32 -> u32>                        0x60aa60
/// cont<u32 -> element>                    0x630ab0
/// cont<u32 -> u32>                        0x5f9f70
/// cont<ID32>, cont<ID32>                  0x970930 (IdentityLists)
/// [0x67d240 if flag +0xf1]
/// u32 n,   n x (u32, u64, u64)
/// u32 n,   n x element                    0x5dafa0
/// u32 n,   n x u32
/// cont<u32 -> 5 x u32>                    0xa83b30
/// u32 n,   n x (u32, u32)
/// u32 n,   n x (u32, u32)
/// cont<u32 -> element>                    0xe43450
/// ```
fn full_player_character(id: Id32, c: &Character, collection: &Collection) -> bytes::Bytes {
    let proto = ProtoFullCharacterIir {
        // the earned tags (achievements, tutorial, story): live sent them in every copy (recorded
        // teleports); the client forgets those it had when the character comes anew, so without
        // them every achievement looked locked after a teleport
        character_tag: c.tags.keys().copied().collect(),
        inventory: crate::collection::inventories(id.instance, &c.team, collection, &|t| c.item_state(t)),
        ..Default::default()
    };
    iir::record("FullPlayerCharacterIIR", id, false, |w| {
        w.proto(&proto)
            .u32(FULL_CHARACTER_VERSION)
            .u32(0)
            .u32(EMPTY)
            .u32(0)
            .u32(EMPTY)
            .u32(EMPTY)
            .u32(EMPTY)
            .u32(EMPTY)
            .u32(EMPTY)
            .u32(0)
            .u32(0)
            .u32(0)
            .u32(EMPTY)
            .u32(0)
            .u32(0)
            .u32(EMPTY)
    })
}

const START_HEALTH: u32 = 100;

/// MonsterData (body and costume, RDB 1010514) for a minifigure item template, from
/// content/minifigures `render.monster_data` (tools/re/extract_minifig_monster_data.py: each template's
/// SetStatGameAction on stat 359). 2212, the value in the templates' own stat block, is only the
/// generic body ("Aztec_Warrior"). `CoreCharacterFullUpdateIIR::Transfer` applies
/// `stats.monster_data` before building the animated mesh; without it characters are invisible.
fn monster_data_for(template: u32) -> u32 {
    crate::minifigs::monster_data(template).unwrap_or(2212)
}

/// Engine stat ids (data/stats.json, from the client's stat table at 0x1607bd0).
pub const STAT_LIFE: u32 = 1;
pub const STAT_HEALTH: u32 = 27;
/// BCCState; 29 is Limbo (dead: death effect, "CombatChat_CharacterKilled").
pub const STAT_BCC_STATE: u32 = 20;
pub const BCC_STATE_LIMBO: u32 = 29;
const BCC_STATE_NORMAL: u32 = 1;
const STAT_LEVEL: u32 = 54;
const STAT_XP: u32 = 52;
const STAT_LAST_XP: u32 = 57;
const STAT_NEXT_XP: u32 = 350;
const STAT_GM_LEVEL: u32 = 215;
const STAT_CHAR_RADIUS: u32 = 421;
const STAT_MONSTER_DATA: u32 = 359;
/// Active minifigure team slot (inventory 101 position), client name e_ActiveWeapon.
const STAT_ACTIVE_WEAPON: u32 = 2000405;
pub const TOKEN_HEALTH_POTIONS: u32 = 150;
/// Minifigure XP, shown as the star counter (top left).
const TOKEN_STARS: u32 = 100;
/// Inventory of minifigure bags (dynel type 34001 items; ItemHolder::GetItemTypeInventory).
pub const MINIFIGURE_BAG_INVENTORY: u32 = 110002;
/// Stars for a minifigure bag of an already unlocked minifigure ("Duplicates will be rewarded
/// with stars"; the amount is a guess).
const DUPLICATE_BAG_STARS: u64 = 50;
/// Stars per spare minifigure part converted. Live capture 2016: converting 3 parts gave a
/// 250-star bag (one sample), so about 83 a part; whether live paid per part or per conversion is
/// unknown.
const STARS_PER_PART: u64 = 83;

/// ItemShopCurrencies(i32 methodId, vector<ItemShopCurrency>) as live sent it: methodId 2, then
/// u32 id, u32 amount, string name per currency (the ids of the last four varied per login; these
/// are the first recorded ones). STARS carries the star count.
fn item_shop_currencies(stars: u64) -> Bytes {
    const CURRENCIES: [(u32, &str); 6] =
        [(1, "VOUCHERS"), (2, "STARS"), (0x39, "GAME_VOUCHER"), (0x3a, "PAID_DIAMONDS"), (0x3b, "BONUS_DIAMONDS"), (0x3c, "GAME_DIAMONDS")];
    let w = Writer::new().i32(2).container_len(CURRENCIES.len());
    CURRENCIES
        .iter()
        .fold(w, |w, &(id, name)| w.u32(id).u32(if id == 2 { stars.min(u64::from(u32::MAX)) as u32 } else { 0 }).string(name))
        .finish()
}

/// Brings a player whose whole team was smashed back: out of Limbo, at `spot` or the respawn
/// point nearest to `here` (a teleport within the playfield, which also sends the team again).
fn respawn(id: Id32, me: &Character, here: [f32; 3], spot: Option<&crate::teleport::Spot>, outbox: &world::Outbox) {
    world::send_playfield(id, true, world::Out::Records(vec![stat_record(id, STAT_BCC_STATE, BCC_STATE_NORMAL)]));
    match spot.or_else(|| crate::teleport::nearest_spot(me.playfield, here)) {

        Some(s) => {
            info!(character = id.instance, guid = s.guid, "respawn after the team was smashed");
            let _ = outbox.send(world::Out::Teleport(None, s.pos, s.rot));
        }
        None => warn!(character = id.instance, playfield = me.playfield, "no respawn point"),
    }
}

/// Rebuilds smashed minifigure `template` at `until` (unix ms), through the session's outbox.
fn schedule_rebuild(outbox: &world::Outbox, template: u32, until: i64) {
    let outbox = outbox.clone();
    tokio::spawn(async move {
        tokio::time::sleep(crate::smash::left(until)).await;
        let _ = outbox.send(world::Out::Rebuilt(template));
    });
}

/// A smashed minifigure back at full health: the server's health, then UnlockTemplate (the
/// client restores its health share and lets it be picked), and the active one's health.
async fn rebuild(conn: &mut Conn, character: u32, me: &Character, template: u32) -> anyhow::Result<()> {
    let id = Id32::new(crate::territory::CHARACTER_TYPE, character);
    info!(character, template, "minifigure rebuilt");
    let active = crate::npc::rebuild(id, template);
    conn.send_as(Some(character), Endpoint::GameCharInterface, game_char_interface::UNLOCK_TEMPLATE, crate::smash::unlock_args(template))
        .await?;
    if active.is_some() {
        let records = health_records(id, template, me.upgrade(template), me.level());
        conn.send_as(Some(character), Endpoint::GameserverToClientRPC, client::PASS_BLOB, iir::pass_blob(&records)).await?;
    }
    Ok(())
}

/// UpdateTokenAmount(u32 token, u64 gained, u64 spent): cumulative totals, not deltas.
async fn send_token(conn: &mut Conn, character: u32, token: u32, gained: u64, spent: u64) -> anyhow::Result<()> {
    let args = Writer::new().u32(token).u64(gained).u64(spent).finish();
    conn.send_as(Some(character), Endpoint::GameCharInterface, game_char_interface::UPDATE_TOKEN_AMOUNT, args).await
}
/// A health orb heals 15% of the active minifigure's maximum health, rounded half-up (live
/// capture 2016, 58 samples; live's record wasn't clamped to the maximum, ours is).
const HEALTH_ORB_HEAL_SHARE: f32 = 0.15;
/// Potions a new character starts with (the count is saved; enemies drop more, loot.rs).
pub const START_POTIONS: u64 = 5;
/// "Potion of Health" (in shortcut slot 100): the heal was server-side; it restores the active
/// minifigure to full health (as the user remembers live).
const POTION_SPELL: u32 = 104_554;
const POTION_HEAL_SHARE: f32 = 1.0;
/// Effect package of the potion's heal (hearts on the character).
const POTION_EFFECT: u32 = 4456;
/// BehaviorControlCenterIIR_c (WriteSubClass 0x1a37690): u8 action, u32 CommandType,
/// CommandMessage protobuf, command data. The client queues a command with action 3 and the
/// server answers 4 (OnSuccess), 5 (OnFailure), 7/8/9 (abort one/queued/all).
const BCC_QUEUE_COMMAND: u8 = 3;
const BCC_COMMAND_SUCCEEDED: u8 = 4;
const BCC_COMMAND_FAILED: u8 = 5;
/// Client -> server: a held ability was released (MagicCommand_c::FinishChannel).
const BCC_FINISH_CHANNEL: u8 = 20;
/// A charge nobody released (lost focus...) fires after this long.
const SUMMON_RECHARGE: std::time::Duration = std::time::Duration::from_secs(2);
const CHARGE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(6);
/// How long a fully charged ability is held before it fires by itself (assumed; see
/// docs/TUNABLES.md).
const CHARGE_HOLD: std::time::Duration = std::time::Duration::from_secs(3);
const BCC_CAN_EXECUTE: u8 = 10;
/// Client -> server: the client aborted a command (AbortOneCommand) and waits for the server's
/// confirmation, BCC_ABORT_ONE.
const BCC_CLIENT_ABORT: u8 = 2;
const BCC_ABORT_ONE: u8 = 7;
/// After a charge fired by itself, a still-held button makes the client re-send the cast, which
/// would start a new charge; such re-sends are refused until none came for this long (assumed).
const CHARGE_RECAST_GAP: std::time::Duration = std::time::Duration::from_millis(500);
pub const COMMAND_MAGIC: u32 = 4;
/// Reach of untargeted (melee/area) attacks: a circle of AREA_RADIUS centred MELEE_REACH in
/// front of the character.
const AREA_RADIUS: f32 = 2.5;
const MELEE_REACH: f32 = 1.5;
/// CommandType of CycleToNextWeaponCommand (switch minifigure: u8 flag, u32 slot); 4 = MagicCommand.
const COMMAND_CYCLE_WEAPON: u32 = 0x30;
/// The ObjectInteraction action the client queues after a use (StopUse).
const INTERACTION_STOP_USE: u32 = 0x22;
/// The command that sets the character's BCCState (data: u32 state; building asks for 104).
const COMMAND_BCC_STATE: u32 = 1;
/// EquipItemCommand: a minifigure moved from the collection into a team slot.
const COMMAND_EQUIP_ITEM: u32 = 9;
/// Queued commands allowed (client name "CommandQueueSize"; treated as 1 when unset).
const STAT_COMMAND_QUEUE_SIZE: u32 = 1360;
/// Number of unlocked team slots (client name e_MaxEquippedWeapons).
const STAT_MAX_EQUIPPED_WEAPONS: u32 = 2_000_446;
const STAT_HEALTH_PER_LEVEL: u32 = 3_000_000;

/// Near the portals in playfield 1200 (from the client's RDBTeleportLocations).
const START_POS: [f32; 3] = [320.0, 56.0, 110.0];

/// `SimpleCharFullUpdateIIR_t`: creates the character dynel (it is a GameCodeDynelFullUpdateIIR,
/// i.e. an n3DynelFactoryCallbackObject). Header protobuf `ProtoGameCodeDynelIIR` (position,
/// rotation, playfield). SimpleChar part (0xe0b150): u8 version 4, u8, 5 u32, string, 4 u32,
/// u32 n + n u32, u32 n + n (u32,u32), u32 n + n (u32,u32); then CoreCharacterFullUpdateIIR
/// (0x5f4650): u8 'B', u32, ProtoCoreCharacterIIR (generic_stats must have exactly as many
/// entries as g_Int32StatsOrder, which holds one stat), u32 n + n u32, blob, cont, cont<u32,u8>,
/// cont, u32 n + n (u32,u32), u32 n + n (u32,u32), cont<u32,u32>, cont<u32,u32>.
/// `Int32StatIIR` (ReadSubClass 0x87a940): u32 stat, u32 value. Applying it (0x87b270) calls
/// the dynel's `StatHolder_t` (+0xac) SetStat — the same holder Flash's `GetStat` reads, so this
/// is what the HUD health bar (`GetStat(e_Health) / GetStat(e_Life)`) sees.
pub fn int32_stat(id: Id32, stat: u32, value: u32) -> bytes::Bytes {
    iir::record("Int32StatIIR", id, false, |w| w.u32(stat).u32(value))
}

/// `StatIIR_t`: u32 stat, u32 value. What the original server sent for stat changes (e.g. a
/// destructible's Health 0, which plays its smash right away).
pub fn stat_record(id: Id32, stat: u32, value: u32) -> bytes::Bytes {
    iir::record("StatIIR_t", id, false, |w| w.u32(stat).u32(value))
}

/// What the character IIRs need to know about a character, player or NPC.
pub struct CharSpec<'a> {
    pub name: &'a str,
    pub playfield: Pf,
    pub pos: [f32; 3],
    /// Body and costume (RDB 1010514).
    pub monster_data: u32,
    pub level: u32,
    pub health: u32,
    pub max_health: u32,
    /// CARS target group: 0 for players, 3 for enemies (attackable by players).
    pub cars_group: u32,
    /// Set for NPCs.
    pub npc: Option<proto_core_character_iir::NpcStats>,
    /// Movement speed float stats (stat, m/s), see `PLAYER_SPEEDS`.
    pub speeds: &'static [(u32, f32)],
}

/// Game rules on a saved character (the type lives in lmoxide-db).
pub trait CharacterExt {
    /// Active upgrade bitmask of a minifigure.
    /// The active upgrade bits of a minifigure (only of the tiers its rank unlocked).
    fn upgrade(&self, template: u32) -> u32;
    /// Player level (stat 54).
    fn level(&self) -> u32;
    /// Minifigure rank (item stat MinifigureLevel).
    fn rank(&self, template: u32) -> u32;
    /// The upgrades bought for a minifigure (item stat PurchasedUpgrades): all without
    /// progression; older saves without purchases have their rank's tiers.
    fn purchased(&self, template: u32) -> u32;
    /// What the minifigure's item carries (upgrades, rank, life, achievement counters).
    fn item_state(&self, template: u32) -> crate::collection::ItemState;
    fn spec(&self, place: Pf) -> CharSpec<'_>;
}

impl CharacterExt for Character {
    fn upgrade(&self, template: u32) -> u32 {
        let active = self.upgrades.get(&template).copied().unwrap_or(crate::collection::DEFAULT_ACTIVE);
        active & self.purchased(template)
    }

    fn level(&self) -> u32 {
        if crate::progression::enabled() { self.level.max(1) } else { CHARACTER_LEVEL }
    }

    fn rank(&self, template: u32) -> u32 {
        if crate::progression::enabled() {
            self.minifigure_levels.get(&template).copied().unwrap_or(0)
        } else {
            crate::progression::MAX_RANK
        }
    }

    fn purchased(&self, template: u32) -> u32 {
        if crate::progression::enabled() {
            self.purchased_upgrades.get(&template).copied().unwrap_or_else(|| crate::progression::unlocked_upgrades(self.rank(template)))
        } else {
            crate::progression::unlocked_upgrades(crate::progression::MAX_RANK)
        }
    }

    fn item_state(&self, template: u32) -> crate::collection::ItemState {
        crate::collection::ItemState {
            active: self.upgrade(template),
            rank: (self.rank(template), self.purchased(template)),
            life: crate::combat::item_life(template, self.upgrade(template), self.level()),
            counters: crate::achievements::item_counters(self, template),
        }
    }

    /// How the character appears in `place` (the running copy it is in).
    fn spec(&self, place: Pf) -> CharSpec<'_> {
        CharSpec {
            name: &self.name,
            playfield: place,
            pos: self.pos,
            // the active (first) team slot's minifigure
            monster_data: self.team.first().map_or(2212, |&(_, t)| monster_data_for(t)),
            level: self.level(),
            health: self.team.first().map_or(START_HEALTH, |&(_, t)| crate::combat::max_life(t, self.upgrade(t), self.level())),
            max_health: self.team.first().map_or(START_HEALTH, |&(_, t)| crate::combat::max_life(t, self.upgrade(t), self.level())),
            cars_group: 0,
            npc: None,
            speeds: PLAYER_SPEEDS,
        }
    }
}

/// The speed paths (MoveTaskIIR_c) move characters at.
const STAT_RUN_SPEED: u32 = 1397;
/// Movement speed stats of players in the recorded session (m/s): MovementChargeSpeed 1400,
/// 1402, 1401, 1397, 1398, 1399, 1396. Paths (MoveTaskIIR_c) move characters at 1397 (measured
/// on the recorded NPCs, see npc::RUN_SPEED).
pub const PLAYER_SPEEDS: &[(u32, f32)] =
    &[(1400, 37.5), (1402, 50.0), (1401, 50.0), (1397, 5.6), (1398, 5.6), (1399, 5.6), (1396, 5.6)];

fn simple_char_full_update(id: Id32, c: &CharSpec) -> bytes::Bytes {
    let (playfield, name, pos) = (c.playfield, c.name, c.pos);
    // players' team (the client's SameTeamAsClient); NPCs have none
    let team = if c.npc.is_none() { crate::community::team_ids(id.instance) } else { None };
    let dynel = ProtoGameCodeDynelIir {
        position: Some(ProtoVec3 { x: Some(pos[0]), y: Some(pos[1]), z: Some(pos[2]) }),
        rotation: Some(ProtoQuat { x: Some(0.0), y: Some(0.0), z: Some(0.0), w: Some(1.0) }),
        playfield_instance: Some(playfield.key()),
        running_effects: vec![],
    };
    let core = ProtoCoreCharacterIir {
        name: Some(name.to_string()),
        stats: Some(proto_core_character_iir::CharacterStats {
            life: Some(c.max_health),
            health: Some(c.health),
            level: Some(c.level),
            monster_data: Some(c.monster_data),
            monster_scale: Some(100),
            cars_group: (c.cars_group != 0).then_some(c.cars_group),
            ..Default::default()
        }),
        npc_stats: c.npc.clone(),
        team_id: team.as_ref().map(|t| t.0.clone()),
        team_leader_id: team.map(|t| t.1),
        generic_stats: vec![0],
        dimension_id: Some(1),
        ..Default::default()
    };
    iir::record("SimpleCharFullUpdateIIR_t", id, false, |w| {
        let w = w.proto(&dynel).u8(4).u8(0);
        let w = (0..5).fold(w, |w, _| w.u32(0)).string(name);
        let w = (0..4).fold(w, |w, _| w.u32(0));
        let w = w.u32(0).u32(0);
        // float stats (u32 n, n × (u32 stat, f32)): the movement speeds, as the original server
        // gave every character, among them MovementChargeSpeed (1400), the speed of dashes
        let w = c.speeds.iter().fold(w.u32(c.speeds.len() as u32), |w, &(stat, v)| w.u32(stat).f32(v));
        // Vehicle_t::ReadState stream: u32 state, then (for vehicles that keep one) u8
        // has-move-task + the move task. The recorded session sent 2 + 0 for NPCs and 2 for the
        // player's own character; an empty stream made the client read a move task out of stray
        // memory (garbage types, crashes). Everyone gets the NPC form: it is its own
        // length-prefixed stream, so a vehicle that doesn't read the flag ignores it, and other
        // players' characters may well read it.
        let vehicle = Writer::new().u32(2).u8(0).finish();
        let w = w
            // CoreCharacterFullUpdateIIR
            .u8(b'B')
            .u32(0)
            .proto(&core)
            .u32(0)
            .blob(&vehicle)
            .u32(EMPTY)
            .u32(EMPTY)
            .u32(EMPTY);
        // two (u32, u32) maps (+0x14c, +0x154); their consumer isn't known yet
        w.u32(0).u32(0).u32(EMPTY).u32(EMPTY)
    })
}

/// Field ids in DreamWorld reflection data are the ELF/PJW hash of the field name (0x4ab4a0).
fn elf_hash(name: &str) -> u32 {
    let mut h: u32 = 0;
    for &b in name.as_bytes() {
        h = (h << 4).wrapping_add(b as u32);
        let g = h & 0xf000_0000;
        if g != 0 {
            h ^= g >> 24;
        }
        h &= !g;
    }
    h
}

/// `UpdateMessage` (dw::UpdateMessage, op 0 = create): creates the DreamWorld GameObject that
/// dynel IIRs wait for (GameCodeDynelFullUpdateIIR::PollStatus needs it, n3DynelFactory asserts
/// it). Layout (ReadSubClass 0x6ef680): u8 op, ID32, [op 0: u32 playfield key, blob].
/// The blob is a reflected dw::GameObject (0x6ef090): u32 byte length, then (u32 field id,
/// value) pairs; fields registered in 0x6f3a10: Id (ID32, +0x148), Position (Vector3Member),
/// Rotation, Scale. AddGameObject (0x6edf10) keys the object by its Id. Unlike the outer
/// stream, the reflection data is little-endian (the nested BinaryStream doesn't byte-swap).
/// A reflected object: u32 LE byte length, then (u32 LE field id, value) pairs.
fn reflect(fields: &[(u32, &[u8])]) -> Vec<u8> {
    let mut body = Vec::new();
    for (id, value) in fields {
        body.extend_from_slice(&id.to_le_bytes());
        body.extend_from_slice(value);
    }
    let mut out = (body.len() as u32).to_le_bytes().to_vec();
    out.extend_from_slice(&body);
    out
}

pub fn create_game_object(id: Id32, playfield: Pf, pos: [f32; 3]) -> bytes::Bytes {
    // Only Id: Position is a reflected struct (Vector3Member) and the dynel IIR carries the
    // position anyway. ID32 is itself a reflected struct {type @0, instance @4} (0x6f3e80),
    // so it nests as its own length + fields; u32 fields are plain little-endian values.
    let _ = pos;
    let id_value = reflect(&[(elf_hash("type"), &id.kind.to_le_bytes()), (elf_hash("instance"), &id.instance.to_le_bytes())]);
    let object = reflect(&[(elf_hash("Id"), &id_value)]);
    iir::record("UpdateMessage", id, false, |w| id.write(w.u8(0)).u32(playfield.key()).blob(&object))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_minifigure_moves_to_first_position() {
        let mut team = vec![(20, 41_894), (6, 498_522), (8, 550_251)];
        active_at_first_position(&mut team);
        assert_eq!(team, vec![(6, 41_894), (20, 498_522), (8, 550_251)]);
        let mut team = vec![(6, 1), (8, 2), (20, 3)];
        active_at_first_position(&mut team);
        assert_eq!(team, vec![(6, 1), (8, 2), (20, 3)]);
    }

    #[test]
    fn test_team_keeps_slots_and_active() {
        let (a, b, c, d) = (550_248, 196_023, 595_244, 357_916);
        let mut me = new_character("t".into(), 1200);
        // Revolutionary Soldier (b) active in slot 8, the same team asked for: nothing changes
        me.team = vec![(8, b), (6, a), (20, c)];
        let mut collection = Collection::new(&me.team, me.collection.as_ref());
        set_team(&mut me, &mut collection, &[a, b, c]);
        assert_eq!(me.team, vec![(8, b), (6, a), (20, c)]);
        // a new minifigure takes the leaving one's slot; the active one stays
        set_team(&mut me, &mut collection, &[d, b, c]);
        assert_eq!(me.team, vec![(8, b), (6, d), (20, c)]);
        // the active one leaves: the first newcomer takes its slot and is active
        set_team(&mut me, &mut collection, &[a, d, c]);
        assert_eq!(me.team, vec![(8, a), (6, d), (20, c)]);
    }

    #[test]
    fn elf_hash_matches_client() {
        // classic PJW/ELF hash values
        assert_eq!(elf_hash(""), 0);
        assert_eq!(elf_hash("Id"), (b'I' as u32) * 16 + b'd' as u32);
    }

    #[test]
    fn item_shop_currencies_match_recording() {
        // the first ItemShopCurrencies of the recorded session (0 stars)
        let recorded = "0000000200001b9700000001000000000008564f5543484552530000000200000000000553544152530000003900000000000c47414d455f564f55434845520000003a00000000000d504149445f4449414d4f4e44530000003b00000000000e424f4e55535f4449414d4f4e44530000003c00000000000d47414d455f4449414d4f4e4453";
        assert_eq!(crate::conn::hex(&item_shop_currencies(0)), recorded);
    }

    fn floats(head: &[u8], values: &[f32]) -> Vec<u8> {
        let mut b = head.to_vec();
        values.iter().for_each(|v| b.extend(v.to_be_bytes()));
        b
    }

    #[test]
    fn earned_tags_go_with_the_character_as_live_sent_them() {
        use prost::Message;
        let proto = ProtoFullCharacterIir { character_tag: vec![385, 394, 395, 4121, 4221], ..Default::default() };
        // the recorded FullPlayerCharacterIIR after the tutorial (field 1, packed)
        assert_eq!(crate::conn::hex(&proto.encode_to_vec()), "0a0a81038a038b039920fd20");
    }

    #[test]
    fn movement_reports_tell_where_players_are() {
        let now = std::time::Instant::now();
        // keyboard: yaw-only rotation, then u8 alignment and the velocity (as `move_record`)
        let mut steer = floats(&[0x18], &[1.0, 0.0, 2.0, 0.0, 1.0]);
        steer.extend(floats(&[0], &[4.0, 0.0, 0.0, 0.0]));
        let m = reported_motion("CharDCMoveIIR_t", &steer).unwrap();
        assert_eq!(m.at(now), [1.0, 0.0, 2.0]);
        assert!((m.at(now + std::time::Duration::from_millis(100))[0] - 1.4).abs() < 1e-2);
        // not further than STEER_AHEAD without another report
        assert!((m.at(now + std::time::Duration::from_secs(2))[0] - 2.2).abs() < 1e-2);
        // stopped (no velocity)
        let stop = floats(&[0x1c], &[1.0, 0.0, 2.0, 0.0, 1.0]);
        assert_eq!(reported_motion("CharDCMoveIIR_t", &stop).unwrap().at(now + std::time::Duration::from_secs(1)), [1.0, 0.0, 2.0]);
        // click-to-move: start, then the waypoints at 32 (u32 count at 28), walked at 5.6 m/s
        let mut path = floats(&[0], &[0.0, 0.0, 0.0]);
        path.extend([0x0f, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        path.extend(0.072f32.to_be_bytes());
        path.extend(2u32.to_be_bytes());
        path.extend(floats(&[], &[5.6, 0.0, 0.0, 5.6, 0.0, 5.6]));
        assert_eq!(reported_position("MoveTaskIIR_c", &path), Some([5.6, 0.0, 5.6]));
        let m = reported_motion("MoveTaskIIR_c", &path).unwrap();
        let at = |ms| m.at(now + std::time::Duration::from_millis(ms));
        assert!((at(500)[0] - 2.8).abs() < 1e-3);
        assert!((at(1500)[2] - 2.8).abs() < 1e-3 && (at(1500)[0] - 5.6).abs() < 1e-3);
        assert_eq!(at(5000), [5.6, 0.0, 5.6]);
    }
}

/// Starting team in inventory 101 (positions 6, 8 and 20 are the three team slots).
/// Minifigure item templates 663917..=663932 (listed in a client Lua table of minifigures).
const START_TEAM: [(u32, u32); 3] = [(6, 663929), (8, 663921), (20, 663932)];
/// Character level (stat 54). The Collection's upgrade panel is gated on it (locked below 3,
/// tiers need higher levels), so characters start high.
const CHARACTER_LEVEL: u32 = crate::combat::LEVEL;

pub fn acg_item(template: u32, level: u32) -> ProtoAcgItem {
    ProtoAcgItem {
        template1: Some(template),
        template2: Some(template),
        template3: Some(template),
        level: Some(level),
        ..Default::default()
    }
}

