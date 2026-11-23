//! Small world rules of AE2's blocks: pistons leave the network's cables (and the spatial matrix frame) alone, as
//! Java's cable bus (a block entity) and MatrixFrameBlock (PushReaction.BLOCK) are; sky stone breaks ten times faster
//! with a good enough tool (SkyStoneBreakSpeed).

use mod_api::*;

/// AE2's cable colours (AEColor), with its own transparent fluix.
pub(crate) const COLORS: [&str; 17] = [
    "white",
    "orange",
    "magenta",
    "light_blue",
    "yellow",
    "lime",
    "pink",
    "gray",
    "light_gray",
    "cyan",
    "purple",
    "blue",
    "brown",
    "green",
    "red",
    "black",
    "fluix",
];
pub(crate) const CABLE_KINDS: [&str; 5] = [
    "glass_cable",
    "covered_cable",
    "smart_cable",
    "covered_dense_cable",
    "smart_dense_cable",
];

/// Every cable block (the ones without a block entity a piston would otherwise move).
pub(crate) fn cable_blocks() -> Vec<String> {
    COLORS
        .iter()
        .flat_map(|c| CABLE_KINDS.iter().map(move |k| format!("ae2:{c}_{k}")))
        .collect()
}

/// A block no piston moves.
pub(crate) struct NoPush;

#[mod_api::sunrise_part]
impl BlockPart for NoPush {
    fn piston_push_reaction(&self, _event: &mut PistonPushReactionEvent) -> Option<PushReaction> {
        Some(PushReaction::Immoveable)
    }
}

pub(crate) fn register(r: &mut dyn Registrar) {
    for b in cable_blocks()
        .iter()
        .map(String::as_str)
        .chain(["ae2:matrix_frame"])
    {
        let k = r.block_key(b);
        r.block_part(k, NoPush);
    }
}

/// SkyStoneBreakSpeed: above this speed (an iron pickaxe is 6), sky stone breaks ten times faster.
pub(crate) const SKY_STONE_THRESHOLD: f32 = 7.0;
pub(crate) const SKY_STONE_SCALAR: f32 = 10.0;

pub(crate) fn sky_stone_speed(original: f32) -> Option<f32> {
    (original > SKY_STONE_THRESHOLD).then(|| original * SKY_STONE_SCALAR)
}

/// `player_break_speed`: the block's own speed, raised for sky stone.
pub(crate) fn break_speed(host: &dyn Host, event: &mut PlayerBreakSpeedEvent) {
    let Some(level) = host.level(host.dimension()) else {
        return;
    };
    if level
        .block(host, event.pos)
        .is_some_and(|b| b.id(host) == Some("ae2:sky_stone_block"))
        && let Some(speed) = sky_stone_speed(event.original_speed)
    {
        event.new_speed = event.new_speed.max(speed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eighty_five_cables() {
        let all = cable_blocks();
        assert_eq!(all.len(), 85);
        assert!(all.contains(&"ae2:fluix_smart_dense_cable".to_owned()));
        assert!(all.iter().all(|c| crate::net::cable(c).is_some()));
    }

    #[test]
    fn sky_stone_needs_a_good_tool() {
        assert_eq!(sky_stone_speed(6.0), None);
        assert_eq!(sky_stone_speed(8.0), Some(80.0));
    }
}
