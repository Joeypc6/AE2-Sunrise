//! AE2's own advancement triggers (AdvancementTriggers, AppEngAdvancementTrigger): Network Apprentice, Engineer and
//! Administrator by the channels a network uses (PathingService.getAchievementBracket), Recursive Networking (a
//! storage bus on an interface) and Spatial Explorer (a spatial transition). Upstream's `grant_near` and
//! `network_advancements` (`ae2-horizon/mixins/ae2/me.rs`): granted to the players within 64 blocks.
//!
//! The advancements' files name these triggers (`ae2:network_apprentice`...; `tools/overrides/data/ae2/advancement`),
//! and Sunrise's `PlayerRef::trigger_criterion` grants them.

use crate::util::*;
use mod_api::*;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

/// Something done at a place in a level that earns the players near an advancement's trigger.
static DUE: Mutex<Vec<(&'static str, Pos, &'static str)>> = Mutex::new(Vec::new());
/// Triggers already fired for a player this session (each criterion is granted once anyway).
static FIRED: Mutex<Option<HashSet<(u128, &'static str)>>> = Mutex::new(None);

pub(crate) const APPRENTICE: &str = "network_apprentice";
pub(crate) const ENGINEER: &str = "network_engineer";
pub(crate) const ADMIN: &str = "network_admin";
pub(crate) const RECURSIVE: &str = "recursive_networking";
pub(crate) const SPATIAL: &str = "spatial_explorer";

/// The players near `pos` in `dim` earn trigger `id` (`ae2:<id>`), at that level's next tick.
pub(crate) fn due(dim: &'static str, pos: Pos, id: &'static str) {
    let mut d = DUE.lock().unwrap_or_else(|e| e.into_inner());
    if !d.iter().any(|(a, b, c)| *a == dim && *b == pos && *c == id) && d.len() < 256 {
        d.push((dim, pos, id));
    }
}

pub(crate) fn forget_all() {
    DUE.lock().unwrap_or_else(|e| e.into_inner()).clear();
    *FIRED.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// PathingService.getAchievementBracket: the trigger for a network using `channels` channels (8, 128, 2048), and
/// the ones below it (a big network earns them all).
pub(crate) fn brackets(channels: u32) -> &'static [&'static str] {
    match channels {
        0..8 => &[],
        8..128 => &[APPRENTICE],
        128..2048 => &[APPRENTICE, ENGINEER],
        _ => &[APPRENTICE, ENGINEER, ADMIN],
    }
}

/// AppEngAdvancementTrigger.Instance: no conditions to test.
struct Always;
impl CriterionTest for Always {
    fn test(&self, _criterion: &dyn CriterionContext) -> Result<bool, CriterionTestError> {
        Ok(true)
    }
}

/// The level's tick: once a second its networks' brackets, then what is due here goes to the players near.
pub(crate) fn level_tick(host: &mut dyn ServerHost, level: LevelRef) {
    let dim = host.dimension();
    let (tick, grids) = crate::net::with_net(dim, |n| {
        (
            n.tick,
            n.grids
                .iter()
                .filter(|g| g.online)
                .filter_map(|g| Some((g.root()?, g.channels)))
                .collect::<Vec<_>>(),
        )
    });
    if tick % 20 == 7 {
        for (root, channels) in grids {
            for id in brackets(channels) {
                due(dim, root, id);
            }
        }
    }
    let here: Vec<(Pos, &'static str)> = {
        let mut d = DUE.lock().unwrap_or_else(|e| e.into_inner());
        let mine = d
            .iter()
            .filter(|(a, ..)| *a == dim)
            .map(|(_, p, id)| (*p, *id))
            .collect();
        d.retain(|(a, ..)| *a != dim);
        mine
    };
    if here.is_empty() {
        return;
    }
    let players = level.online_players_owned(&*host).unwrap_or_default();
    for player in players {
        let (Some(uuid), Some(at)) = (player.entity().uuid(host), player.entity().position(&*host))
        else {
            continue;
        };
        for (pos, id) in &here {
            let pos = real(*pos);
            let d2: f64 = (0..3)
                .map(|i| (at[i] - (f64::from(pos[i]) + 0.5)).powi(2))
                .sum();
            if d2 > 64.0 * 64.0 {
                continue;
            }
            let new = FIRED
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get_or_insert_with(HashSet::new)
                .insert((uuid, id));
            if !new {
                continue;
            }
            if let Ok(trigger) = Identifier::parse(&format!("ae2:{id}")) {
                let _ = player.trigger_criterion(host, &trigger, Arc::new(Always));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_brackets() {
        assert!(brackets(7).is_empty());
        assert_eq!(brackets(8), &[APPRENTICE]);
        assert_eq!(brackets(127), &[APPRENTICE]);
        assert_eq!(brackets(128), &[APPRENTICE, ENGINEER]);
        assert_eq!(brackets(2048), &[APPRENTICE, ENGINEER, ADMIN]);
    }

    #[test]
    fn due_once_per_place() {
        forget_all();
        due("minecraft:overworld", [1, 2, 3], RECURSIVE);
        due("minecraft:overworld", [1, 2, 3], RECURSIVE);
        assert_eq!(DUE.lock().unwrap().len(), 1);
        forget_all();
    }
}
