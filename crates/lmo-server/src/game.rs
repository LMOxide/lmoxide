//! Game server: the playfield connection the client opens after `LoginConnectReady`
//! (`ClientToGameserverRPC` / `GameserverToClientRPC`, then the gamecode endpoints).

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use bytes::Bytes;

use lmo_net::{
    Endpoint, Reader, Writer, iir,
    methods::{
        client_to_gameserver_rpc as agent, game_char_agent, game_char_agent_project, game_char_interface,
        game_char_interface_project,
        gameserver_to_client_rpc as client,
    },
    proto::{
        PlayfieldProxyData, ProtoAcgItem, ProtoCoreCharacterIir, ProtoFullCharacterIir,
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
    store::{Character, Store},
    world,
};

pub async fn handle(mut conn: Conn, cfg: Arc<Config>) -> anyhow::Result<()> {
    conn.local = Endpoint::ClientToGameserverRPC;
    let status: i32 = std::env::var("LMO_GAME_AUTH_STATUS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    let store = Store::new(&cfg.data_dir);
    let mut character = 0;
    let mut me = Character {
        name: String::new(),
        playfield: cfg.start_playfield,
        pos: START_POS,
        team: START_TEAM.to_vec(),
        stars: 0,
        upgrades: Default::default(),
    };
    let mut collection = Collection::new(&me.team);
    let pos = Arc::new(Mutex::new(START_POS));
    let mut last_save = std::time::Instant::now();
    let mut potions_spent: u64 = 0;
    let mut shown_portal: Option<u32> = None;
    // direction of the player's last movement (aims projectiles cast without a target)
    let mut heading = [0.0f32, 1.0];
    // held charge abilities by command client id, fired on release
    let mut charges: HashMap<u32, Charge> = HashMap::new();
    let mut last_summon: Option<(u32, std::time::Instant)> = None;
    let (outbox, mut inbox) = mpsc::unbounded_channel::<world::Out>();
    let mut presence: Option<world::Presence> = None;
    loop {
        let rpc = tokio::select! {
            rpc = conn.recv() => match rpc? {
                Some(rpc) => rpc,
                None => break,
            },
            Some(out) = inbox.recv() => {
                match out {
                    world::Out::Records(records) => {
                        let blob = iir::pass_blob(&records);
                        conn.send_as(Some(character), Endpoint::GameserverToClientRPC, client::PASS_BLOB, blob).await?;
                    }
                }
                continue;
            }
        };
        let mut r = Reader::new(&rpc.args);
        match (rpc.target(), rpc.method()) {
            (Some(Endpoint::ClientToGameserverRPC), agent::AUTHENTICATE2) => {
                character = r.u32()?;
                let cookie = r.u32()?;
                let version = r.string()?;
                let valid = cookie == crate::universe::cookie_for(character);
                me = store.load(character).unwrap_or_else(|| Character {
                    name: crate::sessions::name(character).unwrap_or_else(|| "Player".into()),
                    ..me.clone()
                });
                *pos.lock().unwrap() = me.pos;
                collection = Collection::new(&me.team);
                info!(character, cookie, %version, valid, name = %me.name, playfield = me.playfield, pos = ?me.pos, "Authenticate2");
                let args = Writer::new().i32(status).finish();
                conn.send_as(Some(character), Endpoint::GameserverToClientRPC, client::ACK_AUTHENTICATE, args)
                    .await?;
                // Characters are keyed by account instance for now (see territory.rs).
                if !crate::sessions::territory_closed(character, std::time::Duration::from_secs(10)).await {
                    warn!(character, "territory connection still open; sending the world anyway");
                }
                // Give the client a moment to process the territory disconnect.
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                send_world(&mut conn, &cfg, character, &me, &collection, potions_spent).await?;
            }
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
                        records.push(crate::teleport::journal_record(h.identity));
                        let refresh = crate::collection::collection_refresh(character, &collection, &|t| me.upgrade(t));
                        records.push(iir::record("UpdateInventoryIIR_t", h.identity, false, |w| w.proto(&refresh)));
                        if let Some(&(slot, template)) = me.team.first() {
                            // ParentStatChanged only reacts to a different slot, and the client
                            // may already assume this one: clear it first.
                            records.push(int32_stat(h.identity, STAT_ACTIVE_WEAPON, 0));
                            records.push(int32_stat(h.identity, STAT_ACTIVE_WEAPON, slot));
                            records.extend(health_records(h.identity, template));
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
                        if presence.is_none() {
                            let (id, snapshot, pos_handle) = (h.identity, me.clone(), pos.clone());
                            let pos = pos.clone();
                            let spawn = move || remote_spawn_records(id, &Character { pos: *pos.lock().unwrap(), ..snapshot.clone() }.spec());
                            presence = Some(world::join(id, me.playfield, pos_handle, spawn, outbox.clone()));
                        }
                    }
                    Some(kind @ ("CharDCMoveIIR_t" | "MoveTaskIIR_c")) => {
                        if let Some(yaw) = (kind == "CharDCMoveIIR_t").then(|| reported_facing(rest)).flatten() {
                            heading = yaw;
                        }
                        if let Some(p) = reported_position(kind, rest) {
                            let before = *pos.lock().unwrap();
                            let (dx, dz) = (p[0] - before[0], p[2] - before[2]);
                            let moved = (dx * dx + dz * dz).sqrt();
                            if kind == "MoveTaskIIR_c" && moved > 0.2 {
                                heading = [dx / moved, dz / moved];
                            }
                            *pos.lock().unwrap() = p;
                            // Hub portals: the client sends nothing, so offer the destination's
                            // teleport window when the player steps into one (once per visit), or
                            // teleport directly if it has none.
                            match crate::teleport::portal_at(me.playfield, p) {
                                Some(to) if shown_portal != Some(to) => {
                                    shown_portal = Some(to);
                                    info!(to, "portal");
                                    match crate::teleport::destination(to).map(|d| d.window).filter(|&w| w != 0) {
                                        Some(window) => {
                                            let args = Writer::new().u32(window).i32(0).finish();
                                            conn.send_as(
                                                Some(character),
                                                Endpoint::GameCharInterfaceProject,
                                                game_char_interface_project::SCRY_SHOW_TELEPORT_MESSAGE_BOX,
                                                args,
                                            )
                                            .await?;
                                        }
                                        None => {
                                            teleport(&mut conn, &cfg, &store, character, &mut me, &collection, &mut presence, &pos, to, potions_spent)
                                                .await?;
                                        }
                                    }
                                }
                                Some(_) => {}
                                None => shown_portal = None,
                            }
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
                        if let Some(c) = parse_command(rest).and_then(|(m, _)| m.commandqueue_client_id).and_then(|id| charges.remove(&id)) {
                            fire_charge(&c, &me, h.identity, *pos.lock().unwrap(), heading);
                        }
                    }
                    Some("BehaviorControlCenterIIR_c") if rest.first() == Some(&BCC_QUEUE_COMMAND) => {
                        // A queued command (minifigure swap, ability, ...). It completes only
                        // when the server answers with the same record as action 4 (success).
                        let command_type = rest.get(1..5).map_or(0, |b| u32::from_be_bytes(b.try_into().unwrap()));
                        let mut refused = false;
                        let mut extra = Vec::new();
                        match parse_command(rest) {
                            Some((msg, data)) => {
                                let target = msg.target.as_ref().map(|t| (t.r#type.unwrap_or(0), t.instance.unwrap_or(0)));
                                // MagicCommand data: u32, u32 spell, ...
                                let spell = (command_type == COMMAND_MAGIC)
                                    .then(|| data.get(4..8).map(|b| u32::from_be_bytes(b.try_into().unwrap())))
                                    .flatten();
                                info!(command_type, ?spell, ?target, client_id = ?msg.commandqueue_client_id, data = %crate::conn::hex(data), "command");
                                if spell == Some(POTION_SPELL) {
                                    // Potion of Health: spends a potion (token 150) and heals.
                                    if potions_spent >= START_POTIONS {
                                        refused = true;
                                    } else {
                                        potions_spent += 1;
                                        let args = Writer::new().u32(TOKEN_HEALTH_POTIONS).u64(START_POTIONS).u64(potions_spent).finish();
                                        conn.send_as(Some(character), Endpoint::GameCharInterface, game_char_interface::UPDATE_TOKEN_AMOUNT, args)
                                            .await?;
                                        let health = crate::npc::heal_player(h.identity, POTION_HEAL);
                                        extra.push(int32_stat(h.identity, STAT_HEALTH, health));
                                        // the heal visual (hearts), shown to everyone
                                        let effect = crate::effects::effect_package(h.identity, POTION_EFFECT);
                                        world::send_playfield(h.identity, false, world::Out::Records(vec![effect.clone()]));
                                        extra.push(effect);
                                    }
                                } else if let Some(spell) = spell {
                                    let special = me.team.first().is_some_and(|&(_, t)| crate::collection::spells(t).get(1) == Some(&spell));
                                    let damage = if special { SPECIAL_DAMAGE } else { NORMAL_DAMAGE };
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
                                            crate::npc::summon(h.identity, me.playfield, spell, at);
                                        }
                                    } else if crate::projectile::params(spell).is_some() {
                                        // a projectile: aim at the target, the clicked point or
                                        // where the player last moved; its hits deal the damage
                                        let here = *pos.lock().unwrap();
                                        let npc_target = target_id.filter(|t| *t != h.identity && crate::npc::is_npc(*t));
                                        let towards = npc_target
                                            .and_then(crate::npc::position)
                                            .or(ground)
                                            .unwrap_or([here[0] + heading[0], here[1], here[2] + heading[1]]);
                                        let charge = crate::projectile::params(spell).is_some_and(|p| !p.charge_items.is_empty());
                                        if charge {
                                            // hold to charge: fired on release (BCC action 20)
                                            fire_stale_charges(&mut charges, &me, h.identity);
                                            if let Some(client_id) = msg.commandqueue_client_id {
                                                let active = Arc::new(std::sync::atomic::AtomicBool::new(true));
                                                show_channel(h.identity, spell, active.clone());
                                                charges.insert(client_id, Charge {
                                                    spell,
                                                    started: std::time::Instant::now(),
                                                    active,
                                                    target: npc_target,
                                                    damage,
                                                });
                                            }
                                        } else {
                                            crate::projectile::launch(h.identity, me.playfield, spell, None, here, towards, npc_target, damage);
                                        }
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
                                            crate::npc::hit_area(h.identity, me.playfield, center, AREA_RADIUS, spell, damage);
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
                        records.extend(extra);
                        if command_type == COMMAND_MAGIC && !refused {
                            // A MagicCommand only executes (and later finishes, freeing the queue
                            // for the next cast) once the server allows it: action 10 sets
                            // m_bCanExecute (MagicCommand_c::CanExecute, RunCommand 0x136d280).
                            reply[action_at] = BCC_CAN_EXECUTE;
                            records.push(Bytes::from(reply));
                        }
                        if command_type == COMMAND_CYCLE_WEAPON && rest.len() >= 4 {
                            // CycleToNextWeaponCommand: subclass ends with the u32 team slot
                            let slot = u32::from_be_bytes(rest[rest.len() - 4..].try_into().unwrap());
                            if let Some(i) = me.team.iter().position(|&(s, _)| s == slot) {
                                me.team[..=i].rotate_right(1);
                                records.push(int32_stat(h.identity, STAT_ACTIVE_WEAPON, slot));
                                records.extend(active_minifigure_records(h.identity, me.team[0].1, &presence));
                            }
                        }
                        if command_type == COMMAND_EQUIP_ITEM
                            && let Some((_, data)) = parse_command(rest)
                            && let Some(new_active) = equip(&mut me, &mut collection, data)
                        {
                            // EquipItemCommand: the client moves the item itself on success
                            records.extend(active_minifigure_records(h.identity, new_active, &presence));
                        }
                        if command_type == COMMAND_EQUIP_ITEM {
                            learn_team_spells(&mut conn, character, &me).await?;
                        }
                        let blob = iir::pass_blob(&records);
                        conn.send_as(Some(character), Endpoint::GameserverToClientRPC, client::PASS_BLOB, blob).await?;
                    }
                    Some("CoreProjectileIIR") => crate::projectile::report(h.identity, blob, rest),
                    Some("MoveInventoryItemIIR_t") => {
                        // Reordering the team (101 -> 101). The client already moved it locally;
                        // echoing would move it back.
                        let mut pr = Reader::new(rest);
                        let len = pr.u32()? as usize;
                        let mv = <lmo_net::proto::ProtoMoveInventoryItemIir as prost::Message>::decode(pr.bytes(len, "move")?)?;
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
                            let records = active_minifigure_records(h.identity, template, &presence);
                            conn.send_as(Some(character), Endpoint::GameserverToClientRPC, client::PASS_BLOB, iir::pass_blob(&records))
                                .await?;
                        }
                    }
                    _ => info!(iir = h.name().unwrap_or("?"), identity = ?h.identity, rest = %crate::conn::hex(rest), "client IIR"),
                }
            }
            (Some(Endpoint::GameCharAgent), game_char_agent::TELEPORT_ME) => {
                // TeleportMe(uint32 playfieldID, uint32 spawnPointID): the world map
                let playfield = r.u32()?;
                let spawn = r.u32()?;
                info!(playfield, spawn, "TeleportMe");
                teleport(&mut conn, &cfg, &store, character, &mut me, &collection, &mut presence, &pos, playfield, potions_spent)
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
            (Some(Endpoint::GameCharAgentProject), game_char_agent_project::TELEPORT_MESSAGE_BOX_ANSWER) => {
                // TeleportMessageBoxAnswer(int answer, int ctx): the playfield (high bit: "enter
                // existing"), or -1 for cancel
                let answer = r.i32()?;
                info!(answer, "teleport window answer");
                if answer > 0 || answer < -1 {
                    let playfield = (answer as u32) & 0x7fff_ffff;
                    teleport(&mut conn, &cfg, &store, character, &mut me, &collection, &mut presence, &pos, playfield, potions_spent)
                        .await?;
                }
            }
            (Some(Endpoint::GameCharAgentProject), game_char_agent_project::ACTIVATE_MINIFIGURE_UPGRADE) => {
                // ActivateMinifigureUpgrade(int minifigureID, int position)
                let template = r.i32()? as u32;
                let position = r.i32()? as u32;
                let old = me.upgrade(template);
                let active = crate::collection::activate(old, position);
                me.upgrades.insert(template, active);
                info!(template, position, active, "activate upgrade");
                let (inv, pos) = match me.team.iter().find(|&&(_, t)| t == template) {
                    Some(&(slot, _)) => (crate::collection::TEAM_INVENTORY, slot),
                    None => (crate::collection::COLLECTION_INVENTORY, collection.position_of(template).unwrap_or(0)),
                };
                // SetItemStat(const ID32& inventory, int pos, int stat, int value)
                let args = Id32::new(inv, character)
                    .write(Writer::new())
                    .i32(pos as i32)
                    .i32(crate::collection::STAT_ACTIVE_UPGRADES as i32)
                    .i32(active as i32)
                    .finish();
                conn.send_as(Some(character), Endpoint::GameCharInterface, game_char_interface::SET_ITEM_STAT, args).await?;
                if inv == crate::collection::TEAM_INVENTORY {
                    // ApplyMinifigureUpgradesOnClient(uint32 minifigure, int gameActionLists, int
                    // activatedUpgrades): remove the old upgrades' effects (3), apply the new (1)
                    for (lists, mask) in [(3, old), (1, active)] {
                        let args = Writer::new().u32(template).i32(lists).i32(mask as i32).finish();
                        conn.send_as(
                            Some(character),
                            Endpoint::GameCharInterfaceProject,
                            game_char_interface_project::APPLY_MINIFIGURE_UPGRADES_ON_CLIENT,
                            args,
                        )
                        .await?;
                    }
                }
                let args = Writer::new().u32(template).i32(0).finish();
                conn.send_as(
                    Some(character),
                    Endpoint::GameCharInterfaceProject,
                    game_char_interface_project::SIGNAL_MINIFIGURE_UPGRADE_RESPONSE,
                    args,
                )
                .await?;
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
                if let Some(p) = &presence {
                    for other in world::neighbours(p.id()) {
                        crate::chat::vicinity(other.instance, character, &me.name, mood, &message);
                    }
                    if cfg.bot {
                        // the test bot answers, so incoming chat can be seen with one client
                        crate::chat::vicinity(character, BOT_INSTANCE, "Bot", 0, &format!("you said: {message}"));
                    }
                }
            }
            (Some(Endpoint::GameCharAgent), game_char_agent::TRY_LOOT_ITEM) => {
                // TryLootItem(const ID32& bag, uint32 itemPos, const ID32& char, uint32 desiredPos):
                // the client auto-collected a dropped star or orb.
                let bag = Id32::read(&mut r)?;
                let item_pos = r.u32()?;
                let id = Id32::new(crate::territory::CHARACTER_TYPE, character);
                match crate::loot::take(id, bag, item_pos) {
                    Some(crate::loot::STAR_ITEM) => {
                        // stars (token 100): cumulative totals, not deltas
                        me.stars += 1;
                        let args = Writer::new().u32(TOKEN_STARS).u64(me.stars).u64(0).finish();
                        conn.send_as(Some(character), Endpoint::GameCharInterface, game_char_interface::UPDATE_TOKEN_AMOUNT, args)
                            .await?;
                    }
                    Some(crate::loot::HEALTH_ORB_ITEM) => {
                        let health = crate::npc::heal_player(id, HEALTH_ORB_HEAL);
                        let blob = iir::pass_blob(&[int32_stat(id, STAT_HEALTH, health)]);
                        conn.send_as(Some(character), Endpoint::GameserverToClientRPC, client::PASS_BLOB, blob).await?;
                    }
                    _ => {}
                }
            }
            // Client-side bookkeeping that needs no answer (to be persisted later).
            (
                Some(Endpoint::GameCharAgent),
                game_char_agent::TUTORIAL_SHOWN
                | game_char_agent::ADD_SHORTCUT
                | game_char_agent::REMOVE_SHORTCUT
                | game_char_agent::SET_PLAYER_GUI_LAYOUT_MODE,
            ) => {}
            _ => warn!(args = %crate::conn::hex(&rpc.args), "unhandled"),
        }
    }
    if character != 0 {
        crate::npc::player_left(Id32::new(crate::territory::CHARACTER_TYPE, character));
        me.pos = *pos.lock().unwrap();
        match store.save(character, &me) {
            Ok(()) => info!(character, pos = ?me.pos, "character saved"),
            Err(e) => warn!(character, "saving character failed: {e:#}"),
        }
    }
    Ok(())
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
    cfg: &Config,
    character: u32,
    me: &Character,
    collection: &Collection,
    potions_spent: u64,
) -> anyhow::Result<()> {
    let id = Id32::new(crate::territory::CHARACTER_TYPE, character);
    let mut records = vec![playfield_full_update(me.playfield)];
    records.extend(spawn_records(id, &me.spec()));
    records.insert(3, full_player_character(id, me, collection));
    if cfg.gm_level != 0 {
        records.push(int32_stat(id, STAT_GM_LEVEL, cfg.gm_level));
    }
    conn.send_as(Some(character), Endpoint::GameserverToClientRPC, client::PASS_BLOB, iir::pass_blob(&records)).await?;
    for (token, gained, spent) in [(TOKEN_HEALTH_POTIONS, START_POTIONS, potions_spent), (TOKEN_STARS, me.stars, 0)] {
        let args = Writer::new().u32(token).u64(gained).u64(spent).finish();
        conn.send_as(Some(character), Endpoint::GameCharInterface, game_char_interface::UPDATE_TOKEN_AMOUNT, args).await?;
    }
    Ok(())
}

/// Moves the player to `playfield` (arriving at its destination point, see teleport.rs):
/// IntendedTeleportIIR_c + n3TeleportIIR_t, leave the old playfield's world, then the new
/// playfield as at login. The client answers with CharInPlayIIR_t, which joins the new world.
#[allow(clippy::too_many_arguments)]
async fn teleport(
    conn: &mut Conn,
    cfg: &Config,
    store: &Store,
    character: u32,
    me: &mut Character,
    collection: &Collection,
    presence: &mut Option<world::Presence>,
    pos: &world::Position,
    playfield: u32,
    potions_spent: u64,
) -> anyhow::Result<()> {
    let Some(dest) = crate::teleport::destination(playfield) else {
        warn!(playfield, "no destination data for playfield");
        return Ok(());
    };
    info!(from = me.playfield, to = playfield, name = %dest.name, "teleport");
    let id = Id32::new(crate::territory::CHARACTER_TYPE, character);
    let records = [
        crate::teleport::intended_teleport_record(id, playfield),
        crate::teleport::teleport_record(id, playfield, dest.pos, dest.rot),
    ];
    conn.send_as(Some(character), Endpoint::GameserverToClientRPC, client::PASS_BLOB, iir::pass_blob(&records)).await?;
    // leaving: despawns the character for everyone in the old playfield
    *presence = None;
    me.playfield = playfield;
    me.pos = dest.pos;
    *pos.lock().unwrap() = dest.pos;
    if let Err(e) = store.save(character, me) {
        warn!(character, "saving character failed: {e:#}");
    }
    // let the client tear down the old playfield first
    tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
    send_world(conn, cfg, character, me, collection, potions_spent).await
}

/// A charge ability being held (see projectile.rs `charge_items`).
struct Charge {
    spell: u32,
    started: std::time::Instant,
    /// Cleared on release: stops re-applying the channel buff.
    active: Arc<std::sync::atomic::AtomicBool>,
    target: Option<Id32>,
    damage: u32,
}

/// While a charge is held, shows its channel buff (the growing orb) on the caster, re-applied
/// every channel interval (MagicBuffIIR_c; the buff lasts 0.75 s).
fn show_channel(caster: Id32, spell: u32, active: Arc<std::sync::atomic::AtomicBool>) {
    let Some(p) = crate::projectile::params(spell) else { return };
    let Some(buff) = p.channel_buff else { return };
    let interval = std::time::Duration::from_secs_f32(p.channel_interval.max(0.1));
    tokio::spawn(async move {
        while active.load(std::sync::atomic::Ordering::Relaxed) {
            let add = crate::effects::buff_add(caster, buff, caster, 75);
            world::send_playfield(caster, true, world::Out::Records(vec![add]));
            tokio::time::sleep(interval).await;
        }
        let remove = crate::effects::buff_remove(caster, buff, caster);
        world::send_playfield(caster, true, world::Out::Records(vec![remove]));
    });
}

/// Fires a released charge: the projectile for the charge level reached.
fn fire_charge(c: &Charge, me: &Character, caster: Id32, here: [f32; 3], heading: [f32; 2]) {
    c.active.store(false, std::sync::atomic::Ordering::Relaxed);
    let Some(p) = crate::projectile::params(c.spell) else { return };
    let held = c.started.elapsed().as_secs_f32();
    let item = p.charged_item(held);
    // The release resends the press-time command, so its point is where the player aimed when
    // they started charging; aim with the facing they have now instead (the client keeps
    // reporting it while they turn). A targeted cast still goes to its target.
    let towards = c.target.and_then(crate::npc::position).unwrap_or([here[0] + heading[0], here[1], here[2] + heading[1]]);
    info!(spell = c.spell, held, item, "charge released");
    crate::projectile::launch(caster, me.playfield, c.spell, Some(item), here, towards, c.target, c.damage);
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
fn active_minifigure_records(id: Id32, template: u32, presence: &Option<world::Presence>) -> Vec<Bytes> {
    let costume = int32_stat(id, STAT_MONSTER_DATA, monster_data_for(template));
    if let Some(p) = presence {
        world::broadcast(p.id(), vec![costume.clone()]);
    }
    info!(template, "active minifigure");
    let mut records = vec![costume];
    records.extend(health_records(id, template));
    records
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
fn equip(me: &mut Character, collection: &mut Collection, data: &[u8]) -> Option<u32> {
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
    (me.team.first().map(|&(s, _)| s) == Some(dst_slot)).then_some(template)
}

/// A queued command from a BehaviorControlCenterIIR_c body: u8 action, u32 CommandType,
/// u32 length + CommandMessage protobuf, then the command's own data.
fn parse_command(rest: &[u8]) -> Option<(lmo_net::proto::CommandMessage, &[u8])> {
    use prost::Message;
    let len = u32::from_be_bytes(rest.get(5..9)?.try_into().ok()?) as usize;
    let msg = lmo_net::proto::CommandMessage::decode(rest.get(9..9 + len)?).ok()?;
    Some((msg, &rest[9 + len..]))
}

/// Makes `template` the active minifigure and sends its health (each minifigure has its own;
/// Life is the maximum).
fn health_records(id: Id32, template: u32) -> Vec<Bytes> {
    let health = crate::npc::set_active_minifigure(id, template);
    vec![int32_stat(id, STAT_LIFE, crate::npc::PLAYER_HEALTH), int32_stat(id, STAT_HEALTH, health)]
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

/// `CharDCMoveIIR_t` (ReadSubClass 0x605e40, direct-control/keyboard movement): u8 flags
/// (bit 4: yaw-only rotation, bits 2/3: no velocity blocks), position, rotation y and w, u8 mode,
/// f32, u8, u8, as the client sends it. Activate (0x605a90) only moves non-local dynels whose
/// movement state allows it; the bot uses `move_task_record` instead, which works.
#[allow(dead_code)]
pub fn move_record(id: Id32, pos: [f32; 3], heading: [f32; 2]) -> Bytes {
    iir::record("CharDCMoveIIR_t", id, false, |w| {
        w.u8(0x1c).f32(pos[0]).f32(pos[1]).f32(pos[2]).f32(heading[0]).f32(heading[1]).u8(0).f32(0.0).u8(0).u8(0)
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

const BOT_INSTANCE: u32 = 0x0b07;

/// A server-side character that circles whoever else is on the playfield, standing in for a
/// second player (`--bot`).
pub async fn run_bot(playfield: u32) {
    let id = Id32::new(crate::territory::CHARACTER_TYPE, BOT_INSTANCE);
    let (outbox, mut inbox) = mpsc::unbounded_channel();
    let pos = Arc::new(Mutex::new(START_POS));
    let spawn_pos = pos.clone();
    let bot = Character { name: "Bot".into(), playfield, pos: START_POS, team: START_TEAM.to_vec(), stars: 0, upgrades: Default::default() };
    let spawn = move || remote_spawn_records(id, &Character { pos: *spawn_pos.lock().unwrap(), ..bot.clone() }.spec());
    let _presence = world::join(id, playfield, pos.clone(), spawn, outbox);
    tokio::spawn(async move { while inbox.recv().await.is_some() {} });
    let mut t = 0.0f32;
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(1000));
    loop {
        tick.tick().await;
        t += 0.6;
        let center = world::someone_else(id, playfield).unwrap_or(START_POS);
        let from = *pos.lock().unwrap();
        let to = [center[0] + 4.0 * t.cos(), center[1], center[2] + 4.0 * t.sin()];
        *pos.lock().unwrap() = to;
        world::broadcast(id, vec![move_task_record(id, from, to)]);
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
fn playfield_full_update(playfield: u32) -> bytes::Bytes {
    let id = Id32::new(PLAYFIELD_TYPE, playfield);
    let proxy = PlayfieldProxyData {
        dimension_id: Some(1),
        model_id: Some(ProtoId32 { r#type: Some(PLAYFIELD_TYPE), instance: Some(playfield) }),
        // key of the client's playfield map ("Inserting playfield N in map"); dynels refer to it
        // via ProtoGameCodeDynelIIR.playfield_instance
        running_instance: Some(playfield),
        ..Default::default()
    };
    iir::record("PlayfieldFIIR_t", id, false, |w| {
        let empty_list = 0u32.to_be_bytes();
        let w = w.u32(0).u32(0).u32(0).proto(&proxy).u32(0).blob(&empty_list).blob(&empty_list);
        (0..6).fold(w, |w, _| w.u32(0))
    })
}

/// Version the client's CoreFullPlayerCharacterIIR reader expects (0x1b81edc).
const FULL_CHARACTER_VERSION: u32 = 24;
/// Empty 1009-encoded container.
const EMPTY: u32 = 0x3f1;

/// `FullPlayerCharacterIIR` (the player's own character), minimal: every list empty.
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
        inventory: crate::collection::inventories(id.instance, &c.team, collection, &|t| c.upgrade(t)),
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
/// data/minifigure_monster_data.json (tools/re/extract_minifig_monster_data.py: each template's
/// SetStatGameAction on stat 359). 2212, the value in the templates' own stat block, is only the
/// generic body ("Aztec_Warrior"). `CoreCharacterFullUpdateIIR::Transfer` applies
/// `stats.monster_data` before building the animated mesh; without it characters are invisible.
fn monster_data_for(template: u32) -> u32 {
    static TABLE: std::sync::LazyLock<HashMap<String, u32>> = std::sync::LazyLock::new(|| {
        serde_json::from_str(include_str!("../../../data/minifigure_monster_data.json")).expect("minifigure_monster_data.json")
    });
    TABLE.get(&template.to_string()).copied().unwrap_or(2212)
}

/// Engine stat ids (data/stats.json, from the client's stat table at 0x1607bd0).
pub const STAT_LIFE: u32 = 1;
pub const STAT_HEALTH: u32 = 27;
/// BCCState; 29 is Limbo (dead: death effect, "CombatChat_CharacterKilled").
pub const STAT_BCC_STATE: u32 = 20;
pub const BCC_STATE_LIMBO: u32 = 29;
const STAT_LEVEL: u32 = 54;
const STAT_GM_LEVEL: u32 = 215;
const STAT_CHAR_RADIUS: u32 = 421;
const STAT_MONSTER_DATA: u32 = 359;
/// Active minifigure team slot (inventory 101 position), client name e_ActiveWeapon.
const STAT_ACTIVE_WEAPON: u32 = 2000405;
const TOKEN_HEALTH_POTIONS: u32 = 150;
/// Minifigure XP, shown as the star counter (top left).
const TOKEN_STARS: u32 = 100;
const HEALTH_ORB_HEAL: u32 = 25;
/// Potions per session (restocked at login).
const START_POTIONS: u64 = 5;
/// "Potion of Health" (in shortcut slot 100); its heal amount was server-side, so pick one.
const POTION_SPELL: u32 = 104_554;
const POTION_HEAL: u32 = 50;
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
const BCC_CAN_EXECUTE: u8 = 10;
pub const COMMAND_MAGIC: u32 = 4;
/// Damage of a minifigure's normal (left-click) and special (right-click) ability until spell
/// data is modelled.
const NORMAL_DAMAGE: u32 = 15;
const SPECIAL_DAMAGE: u32 = 30;
/// Reach of untargeted (melee/area) attacks: a circle of AREA_RADIUS centred MELEE_REACH in
/// front of the character.
const AREA_RADIUS: f32 = 2.5;
const MELEE_REACH: f32 = 1.5;
/// CommandType of CycleToNextWeaponCommand (switch minifigure: u8 flag, u32 slot); 4 = MagicCommand.
const COMMAND_CYCLE_WEAPON: u32 = 0x30;
/// EquipItemCommand: a minifigure moved from the collection into a team slot.
const COMMAND_EQUIP_ITEM: u32 = 9;
/// Queued commands allowed (client name "CommandQueueSize"; treated as 1 when unset).
const STAT_COMMAND_QUEUE_SIZE: u32 = 1360;
/// Number of unlocked team slots (client name e_MaxEquippedWeapons).
const STAT_MAX_EQUIPPED_WEAPONS: u32 = 2_000_446;

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

/// What the character IIRs need to know about a character, player or NPC.
pub struct CharSpec<'a> {
    pub name: &'a str,
    pub playfield: u32,
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
}

impl Character {
    /// Active upgrade bitmask of a minifigure.
    pub fn upgrade(&self, template: u32) -> u32 {
        self.upgrades.get(&template).copied().unwrap_or(crate::collection::DEFAULT_ACTIVE)
    }

    pub fn spec(&self) -> CharSpec<'_> {
        CharSpec {
            name: &self.name,
            playfield: self.playfield,
            pos: self.pos,
            // the active (first) team slot's minifigure
            monster_data: self.team.first().map_or(2212, |&(_, t)| monster_data_for(t)),
            level: CHARACTER_LEVEL,
            health: START_HEALTH,
            max_health: START_HEALTH,
            cars_group: 0,
            npc: None,
        }
    }
}

fn simple_char_full_update(id: Id32, c: &CharSpec) -> bytes::Bytes {
    let (playfield, name, pos) = (c.playfield, c.name, c.pos);
    let dynel = ProtoGameCodeDynelIir {
        position: Some(ProtoVec3 { x: Some(pos[0]), y: Some(pos[1]), z: Some(pos[2]) }),
        rotation: Some(ProtoQuat { x: Some(0.0), y: Some(0.0), z: Some(0.0), w: Some(1.0) }),
        playfield_instance: Some(playfield),
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
        generic_stats: vec![0],
        dimension_id: Some(1),
        ..Default::default()
    };
    iir::record("SimpleCharFullUpdateIIR_t", id, false, |w| {
        let w = w.proto(&dynel).u8(4).u8(0);
        let w = (0..5).fold(w, |w, _| w.u32(0)).string(name);
        let w = (0..4).fold(w, |w, _| w.u32(0));
        let w = w.u32(0)
            .u32(0)
            .u32(0)
            // CoreCharacterFullUpdateIIR
            .u8(b'B')
            .u32(0)
            .proto(&core)
            .u32(0)
            .blob(&[])
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

pub fn create_game_object(id: Id32, playfield: u32, pos: [f32; 3]) -> bytes::Bytes {
    // Only Id: Position is a reflected struct (Vector3Member) and the dynel IIR carries the
    // position anyway. ID32 is itself a reflected struct {type @0, instance @4} (0x6f3e80),
    // so it nests as its own length + fields; u32 fields are plain little-endian values.
    let _ = pos;
    let id_value = reflect(&[(elf_hash("type"), &id.kind.to_le_bytes()), (elf_hash("instance"), &id.instance.to_le_bytes())]);
    let object = reflect(&[(elf_hash("Id"), &id_value)]);
    iir::record("UpdateMessage", id, false, |w| id.write(w.u8(0)).u32(playfield).blob(&object))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elf_hash_matches_client() {
        // classic PJW/ELF hash values
        assert_eq!(elf_hash(""), 0);
        assert_eq!(elf_hash("Id"), (b'I' as u32) * 16 + b'd' as u32);
    }
}

/// Starting team in inventory 101 (positions 6, 8 and 20 are the three team slots).
/// Minifigure item templates 663917..=663932 (listed in a client Lua table of minifigures).
const START_TEAM: [(u32, u32); 3] = [(6, 663929), (8, 663921), (20, 663932)];
/// Character level (stat 54). The Collection's upgrade panel is gated on it (locked below 3,
/// tiers need higher levels), so characters start high.
const CHARACTER_LEVEL: u32 = 50;

pub fn acg_item(template: u32, level: u32) -> ProtoAcgItem {
    ProtoAcgItem {
        template1: Some(template),
        template2: Some(template),
        template3: Some(template),
        level: Some(level),
        ..Default::default()
    }
}

