//! The Homestone (tunables `homestone`): the jail escape's `[Give Homestone]{spawn SLXS 1 m}`
//! (text 823) gives item 443457 "XX_Item that gives Homestone". In the live capture
//! (LMO_Captures.cap, 311.68 s) that was AddItem into the backpack (104) at -1, its ACG item
//! carrying the hash, then AddServerShortcut(2 spell, 103, 155998); the client answered
//! AddShortcut(2, 103, 155998, 0). Live characters had the spell 155998 in their spell list
//! before (LearnSpell after the character's creation); only the shortcut came with the item.
//! LMOxide keeps "item/SLXS" in the character's story progress and sends both on every entry,
//! as the potion's.
//!
//! Casting it: game action 637122 (`ga_teleportplayer`, the LEGO Center's arrival point, 45°;
//! DenyFromEndless). No use of it was captured: the cast time and the particles (5 s) are the
//! nano's ItemDelay and effect package; the refusal in a Lost Creations level is TDB 50018's
//! line, sent as BroadcastMessage (GamecodeClient_t::BroadcastMessage shows a FIFO message).

use bytes::Bytes;
use lmoxide_net::{
    Endpoint, Writer,
    methods::game_char_interface,
    proto::{ProtoInventoryGameEntry, proto_inventory_game_entry::StatValue},
    types::Id32,
};

use crate::{instance::Pf, store::Character};

/// AddServerShortcut / AddShortcut type 2: a spell.
const SPELL_SHORTCUT: i32 = 2;
/// The item's stats as live sent them: Flags 66048 (the template's), 26 = 1, 46 = 13.
const ITEM_STATS: [(u32, u32); 3] = [(0, 66_048), (26, 1), (46, 13)];

fn tunables() -> &'static lmoxide_content::tunables::Homestone {
    &crate::gamedata::get().tunables.homestone
}

/// The story progress key of a character that has it.
fn key() -> String {
    format!("item/{}", tunables().hash)
}

/// The hash that gives it (`!test homestone`).
pub fn hash() -> &'static str {
    &tunables().hash
}

/// Whether `spawn <hash>` gives the Homestone.
pub fn gives(order: &crate::story::Order) -> bool {
    matches!(order, crate::story::Order::Spawn(hash) if !hash.is_empty() && *hash == tunables().hash)
}

pub fn owned(me: &Character) -> bool {
    !tunables().hash.is_empty() && me.story.contains_key(&key())
}

pub fn is_spell(spell: u32) -> bool {
    let t = tunables();
    t.spell != 0 && spell == t.spell
}

/// Gives it: false if the character has it already.
pub fn give(me: &mut Character) -> bool {
    if tunables().hash.is_empty() || owned(me) {
        return false;
    }
    me.story.insert(key(), crate::smash::now_ms() / 1000);
    true
}

/// The item's AddItem: into the backpack at -1 (signalled, not kept), as live.
pub fn add_item(character: u32) -> (Endpoint, u32, Bytes) {
    let t = tunables();
    let mut item = crate::game::acg_item(t.item, 1);
    item.template2 = None;
    item.template3 = None;
    item.hash = Some(hash_value(&t.hash));
    let entry = ProtoInventoryGameEntry {
        item_id: Some(0),
        stat_list: ITEM_STATS.iter().map(|&(stat, value)| StatValue { stat: Some(stat), value: Some(value) }).collect(),
        item_data: Some(item),
        is_static_item: Some(false),
        ..Default::default()
    };
    let args = Id32::new(crate::collection::BACKPACK_INVENTORY, character).write(Writer::new()).i32(-1).proto(&entry).u8(0).finish();
    (Endpoint::GameCharInterface, game_char_interface::ADD_ITEM, args)
}

/// A resource hash as the ACG item's fixed32 holds it ("SLXS": bytes `53 58 4c 53`).
fn hash_value(hash: &str) -> u32 {
    let mut b = [0u8; 4];
    for (i, c) in hash.bytes().take(4).enumerate() {
        b[i] = c;
    }
    u32::from_be_bytes(b)
}

/// On entry: the spell (every character has it), and its shortcut for one that owns the item.
pub fn entry(me: &Character) -> Vec<(Endpoint, u32, Bytes)> {
    let t = tunables();
    if t.spell == 0 {
        return Vec::new();
    }
    let mut out = vec![(Endpoint::GameCharInterface, game_char_interface::LEARN_SPELL, Writer::new().i32(t.spell as i32).finish())];
    if owned(me) {
        out.push(shortcut());
    }
    out
}

/// AddServerShortcut(int type, int position, uint32 template).
pub fn shortcut() -> (Endpoint, u32, Bytes) {
    let t = tunables();
    (Endpoint::GameCharInterface, game_char_interface::ADD_SERVER_SHORTCUT, Writer::new().i32(SPELL_SHORTCUT).i32(t.shortcut as i32).u32(t.spell).finish())
}

/// A cast in `place`: refused in a Lost Creations level (with the line for the player).
pub fn refused(place: Pf) -> Option<(Endpoint, u32, Bytes)> {
    crate::lost_creations::is_level(place.model()).then(|| {
        (Endpoint::GameCharInterface, game_char_interface::BROADCAST_MESSAGE, Writer::new().string(&tunables().denied_text).finish())
    })
}

/// The cast's particles on the caster (for everyone around).
pub fn effect(player: Id32) -> Bytes {
    crate::effects::effect_package(player, tunables().effect_package)
}

pub fn cast_time() -> std::time::Duration {
    tunables().cast.to_std()
}

/// Where it goes: playfield, position, heading.
pub fn destination() -> Option<(u32, [f32; 3], f32)> {
    let t = tunables();
    let world = crate::gamedata::get().worlds.get(&t.world)?;
    Some((world.playfield, t.arrival.pos, t.arrival.rot.map_or(0.0, |r| r.heading())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_item_as_live() {
        // LMO_Captures.cap at 311.68 s (text 823), without the ACG item's three keys (the
        // server's encryption keys; LMOxide sends none, as for the minifigure bags)
        let live_head = "00000068010ab3afffffffff";
        let (_, method, args) = add_item(0x010a_b3af);
        let hex: String = args.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(method, game_char_interface::ADD_ITEM);
        assert!(hex.starts_with(live_head), "{hex}");
        // item id 0, the three stats, template1 443457, level 1, the hash, not static; isItemSplit 0
        let body = "080012060800108084041204081a10011204082e100d1a0c2541c4060038014553584c533800";
        assert!(hex.ends_with(&format!("{body}00")), "{hex}");
    }

    #[test]
    fn shortcut_and_destination() {
        // AddServerShortcut as recorded
        let (_, _, args) = shortcut();
        assert_eq!(&args[..], &[0, 0, 0, 2, 0, 0, 0, 0x67, 0, 2, 0x61, 0x5e]);
        let (pf, pos, rot) = destination().unwrap();
        assert_eq!((pf, pos), (1200, [330.0, 49.0, 67.0]));
        assert!((rot.to_degrees() - 45.0).abs() < 0.01);
        assert_eq!(hash_value("SLXS"), u32::from_le_bytes([0x53, 0x58, 0x4c, 0x53]));
        assert!(gives(&crate::story::Order::Spawn("SLXS".into())));
    }
}
