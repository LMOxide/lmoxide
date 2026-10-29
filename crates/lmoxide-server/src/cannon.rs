//! The cannon in Pirate World (4600), as in the recorded session (recs 29345-30059): once built
//! ("XX_Buildie : Level 02 : 0 No Drop", quest goal "Build a cannon to smash the wall") it fires
//! at the fort wall ("XX_Wall"), which only the cannon breaks (live gave it 100000 health), and
//! the Guard Captain's cinematic ("Cinematic Start Oops", texts 206-211) throws the player in
//! jail (4610). The wall and the cannon come back for the next player.

use crate::instance::Pf;
use std::time::Duration;

use lmoxide_net::types::Id32;
use tracing::info;

use crate::{effects, world};

pub const PLAYFIELD: u32 = 4600;
pub const BUILDIE: &str = "XX_Buildie : Level 02 : 0 No Drop";
const WALL: &str = "XX_Wall";
/// The wall's health (recorded StatIIR 27 just before the cannon's hit).
const WALL_HEALTH: u32 = 100_000;
/// The cannonball: projectile item and spell, straight at 20 m/s from the muzzle (the recorded
/// invisible "XX_Cannon" 656) towards the wall.
const SHOT_ITEM: u32 = 234304;
const SHOT_SPEED: f32 = 20.0;
const SPELL: u32 = 234303;
const MUZZLE: [f32; 3] = [104.91, 71.82, 96.86];
const AIM: [f32; 3] = [111.70, 72.51, 103.47];
/// Effects: the shot (on the muzzle, recorded) and the impact (recorded on an invisible dynel
/// by the wall; played on the wall here).
const SHOT_EFFECT: u32 = 463510;
const IMPACT_EFFECT: u32 = 463511;
/// The Guard Captain's cinematic.
const DIALOGUE: u32 = 206;
/// The wall stays broken through the cinematic, then comes back for the next player
/// (assumed; live kept it broken for the recorded player until the jail teleport).
const WALL_RESPAWN: Duration = Duration::from_secs(120);
/// The built cannon goes back to bricks once the wall is back.
const RESET_AFTER: Duration = Duration::from_secs(125);

/// The wall's health instead of its monster data's (20, a crate's).
pub fn health_override(playfield: u32, name: &str) -> Option<u32> {
    (playfield == PLAYFIELD && name == WALL).then_some(WALL_HEALTH)
}

/// How long the wall stays broken (other smashables: npc.rs RESPAWN).
pub fn respawn(playfield: u32, name: &str) -> Option<Duration> {
    (playfield == PLAYFIELD && name == WALL).then_some(WALL_RESPAWN)
}

/// `player` built the cannon: it fires, the wall falls, the guards come.
pub fn fire(player: Id32, player_name: String) {
    tokio::spawn(async move {
        let Some(cannon) = crate::buildie::find(BUILDIE, Pf::main(PLAYFIELD)) else { return };
        info!(?player, "cannon fires at the wall");
        tokio::time::sleep(Duration::from_millis(500)).await;
        world::send_playfield(cannon, false, world::Out::Records(vec![effects::effect_package(cannon, SHOT_EFFECT)]));
        // (launch_visual starts 1 m above `from`)
        crate::projectile::launch_visual(cannon, Pf::main(PLAYFIELD), SHOT_ITEM, SHOT_SPEED, [MUZZLE[0], MUZZLE[1] - 1.0, MUZZLE[2]], AIM);
        let flight = ((AIM[0] - MUZZLE[0]).powi(2) + (AIM[2] - MUZZLE[2]).powi(2)).sqrt() / SHOT_SPEED;
        tokio::time::sleep(Duration::from_secs_f32(flight)).await;
        // someone else's shot may have broken it already
        if let Some(wall) = crate::npc::find(WALL, Pf::main(PLAYFIELD)) {
            world::send_playfield(wall, false, world::Out::Records(vec![effects::effect_package(wall, IMPACT_EFFECT)]));
            crate::npc::hit(cannon, wall, SPELL, WALL_HEALTH);
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
        if world::whereabouts(player).is_some_and(|(pf, _)| pf == Pf::main(PLAYFIELD))
            && let Some(d) = crate::knubot::starting_with(DIALOGUE)
        {
            // a cinematic: the player speaks for the unspawned trigger NPC
            crate::knubot::start(player, &player_name, player, d);
        }
        tokio::time::sleep(RESET_AFTER).await;
        crate::buildie::reset(cannon);
    });
}
