//! The Item Shop, as far as the Collection needs it: minifigure upgrades are shop items. The
//! upgrade popup (CollectionGUI UpgradeAbilityInfo.as) prices an upgrade with
//! `ItemShopInterface.GetItemWithHash(m_ShopItemID)`, and every upgrade's shop item id is unset
//! in the RDB (1000643), which RDBMinifigureUpgradeData::ReadData defaults to
//! "MinifigureUpgrade<position + 1>": twelve items shared by every minifigure. Buying one sends
//! `RequestItemShopTransaction(3, minifigure template, shop id, currency, amount)`.
//!
//! The upgrade panel (MinifigureUpgradePanel.as) has eight tiers, alternating one upgrade and a
//! choice of two (positions 0, 1-2, 3, 4-5, 6, 7-8, 9, 10-11); tier t opens at the player level
//! of lore tag 466 + t, once an upgrade of the tier before is active. The prices were server
//! data: a tier costs the stars of a minifigure rank step (docs/TUNABLES.md).

use bytes::Bytes;
use lmoxide_net::Writer;

/// ItemShopCurrency id of the stars.
pub const STARS: u32 = 2;
/// Shop ids of the upgrade items: base + position.
const UPGRADE_SHOP_ID: u32 = 9_000;
/// Upgrade positions in the panel.
const UPGRADES: u32 = 12;
/// UpdateItemShopItems / ItemShopReply method ids the client expects.
pub const METHOD_ITEMS: i32 = 3;
pub const METHOD_BOUGHT: i32 = 7;
pub const METHOD_ERROR: i32 = 6;
/// RequestItemShopTransaction request ids: 1 at login (the item list), 3 a purchase.
pub const REQUEST_LIST: u32 = 1;
pub const REQUEST_BUY: u32 = 3;

/// The tier of upgrade `position`.
pub fn tier(position: u32) -> u32 {
    [0, 1, 1, 2, 3, 3, 4, 5, 5, 6, 7, 7].get(position as usize).copied().unwrap_or(u32::MAX)
}

/// The upgrade positions of tier `t`.
fn tier_positions(t: u32) -> &'static [u32] {
    const TIERS: [&[u32]; 8] = [&[0], &[1, 2], &[3], &[4, 5], &[6], &[7, 8], &[9], &[10, 11]];
    TIERS.get(t as usize).copied().unwrap_or(&[])
}

/// Stars an upgrade of tier `t` costs.
pub fn price(t: u32) -> u32 {
    crate::progression::rank_cost(t).unwrap_or(0) as u32
}

/// The upgrade position a shop id sells.
pub fn upgrade_position(shop_id: u32) -> Option<u32> {
    shop_id.checked_sub(UPGRADE_SHOP_ID).filter(|p| *p < UPGRADES)
}

/// Why `position` can't be bought now (`purchased`/`active`: the minifigure's upgrade bits),
/// else its price.
pub fn check(position: u32, purchased: u32, active: u32, player_level: u32, stars: u64) -> Result<u32, &'static str> {
    let t = tier(position);
    if t == u32::MAX {
        return Err("No such upgrade.");
    }
    if purchased & (1 << position) != 0 {
        return Err("You already own this upgrade.");
    }
    if player_level < crate::progression::rank_player_level(t) {
        return Err("Your level is too low for this upgrade.");
    }
    if t > 0 && tier_positions(t - 1).iter().all(|p| active & (1 << p) == 0) {
        return Err("Unlock the previous upgrade first.");
    }
    let price = price(t);
    if stars < u64::from(price) {
        return Err("You don't have enough stars.");
    }
    Ok(price)
}

/// `UpdateItemShopItems(int methodId, vector<ItemShopItem>)` with the upgrade items. An item
/// (operator>>(BinaryStream&, ItemShopItem&), Linux 0x1807281): u32 shop id, u32 template, str
/// hash, u32 effect (6 = a "purchase world" link: not that), str name, short, long description,
/// additional info, u32 icon, large icon, u32, u32 use requirement, u32, vector<ItemCost {u32
/// currency, f32 price, f32 discount (AS3 ItemShopCost: Numbers), u32 stack size}>, vector<str> groups, vector<u32> effects,
/// str url, str, str, u32, u32 required flags (stat 511), u32 flags (1 sale, 2 hot, 3 pending).
pub fn upgrade_items() -> Bytes {
    let mut w = Writer::new().i32(METHOD_ITEMS).container_len(UPGRADES as usize);
    for position in 0..UPGRADES {
        w = w.u32(UPGRADE_SHOP_ID + position).u32(0).string(&format!("MinifigureUpgrade{}", position + 1)).u32(0);
        w = w.string(&format!("Minifigure upgrade {}", position + 1)).string("").string("").string("");
        w = w.u32(0).u32(0).u32(0).u32(0).u32(0);
        w = w.container_len(1).u32(STARS).f32(price(tier(position)) as f32).f32(0.0).u32(1);
        w = w.container_len(0).container_len(0);
        w = w.string("").string("").string("").u32(0).u32(0).u32(0);
    }
    w.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules() {
        assert_eq!(upgrade_position(9_003), Some(3));
        assert_eq!(upgrade_position(9_012), None);
        // tier 0 at level 3 for 50 stars
        assert_eq!(check(0, 0, 0, 3, 50), Ok(50));
        assert!(check(0, 0, 0, 2, 50).is_err());
        assert!(check(0, 0, 0, 3, 49).is_err());
        // tier 1 needs tier 0 active
        assert!(check(1, 0b1, 0, 6, 1000).is_err());
        assert_eq!(check(2, 0b1, 0b1, 6, 1000), Ok(60));
        assert!(check(0, 0b1, 0b1, 30, 1000).is_err());
        assert!(!upgrade_items().is_empty());
    }
}
