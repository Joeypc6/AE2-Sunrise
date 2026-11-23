//! StorageCellTooltipComponent: a storage cell's tooltip shows its installed upgrade cards and what it holds, as item
//! icons with their amounts (`tooltips.showCellUpgrades`, `showCellContent`, `maxCellContentShown`), drawn through
//! Sunrise's tooltip image component (`tooltip_component`, `tooltip_component_draw`).

use crate::cells::{cell_cards, cell_type, new_stack, read_cell, sid, stack_of};
use crate::core::{AeKey, AmountFormat};
use mod_api::*;

/// The icons' step (a slot).
const STEP: f32 = 18.0;

/// What the tooltip shows: the cards, the most-held keys (at most `max`, largest first) and whether more are held.
pub(crate) fn shown(
    keys: &[AeKey],
    amounts: &[i64],
    cards: Vec<String>,
    max: usize,
    show_cards: bool,
    show_content: bool,
) -> (Vec<String>, Vec<(AeKey, i64)>, bool) {
    let mut held: Vec<(AeKey, i64)> = keys
        .iter()
        .cloned()
        .zip(amounts.iter().copied())
        .filter(|(_, n)| *n > 0)
        .collect();
    held.sort_by(|a, b| b.1.cmp(&a.1));
    let more = show_content && held.len() > max;
    held.truncate(if show_content { max } else { 0 });
    (if show_cards { cards } else { Vec::new() }, held, more)
}

fn of_stack(stack: &ItemStack) -> (Vec<String>, Vec<(AeKey, i64)>, bool) {
    if cell_type(sid(stack)).is_none() {
        return (Vec::new(), Vec::new(), false);
    }
    let cfg = crate::config::config();
    let contents = read_cell(stack);
    shown(
        &contents.keys,
        &contents.amounts,
        cell_cards(stack),
        cfg.tooltip_max_cell_content_shown.clamp(1, 32) as usize,
        cfg.tooltip_show_cell_upgrades,
        cfg.tooltip_show_cell_content,
    )
}

/// The component's size: a row of cards, then a row of contents (and "..." for more).
pub(crate) fn size(cards: usize, held: usize, more: bool) -> Option<[f32; 2]> {
    let rows = usize::from(cards > 0) + usize::from(held > 0);
    if rows == 0 {
        return None;
    }
    let wide = cards.max(held + usize::from(more));
    Some([wide as f32 * STEP, rows as f32 * STEP + 2.0])
}

pub(crate) struct CellIcons;

#[mod_api::sunrise_part]
impl ItemPart for CellIcons {
    fn tooltip_component(
        &self,
        host: &dyn ClientHost,
        event: &mut TooltipComponentEvent<'_>,
    ) -> Option<[f32; 2]> {
        crate::cells::ids(host);
        let (cards, held, more) = of_stack(event.stack);
        size(cards.len(), held.len(), more)
    }

    fn tooltip_component_draw(
        &self,
        host: &dyn ClientHost,
        event: &mut TooltipComponentDrawEvent<'_>,
    ) {
        crate::cells::ids(host);
        let (cards, held, more) = of_stack(event.stack);
        let [x0, mut y] = event.origin;
        if !cards.is_empty() {
            for (i, c) in cards.iter().enumerate() {
                if let Some(s) = new_stack(host, c, 1) {
                    event
                        .draw
                        .gui_draw_item_with(&s, [x0 + i as f32 * STEP, y], None);
                }
            }
            y += STEP;
        }
        for (i, (key, n)) in held.iter().enumerate() {
            if let Some(s) = stack_of(host, key, 1) {
                let text = key.format_amount(*n, AmountFormat::Slot);
                event
                    .draw
                    .gui_draw_item_with(&s, [x0 + i as f32 * STEP, y], Some(&text));
            }
        }
        if more {
            event.draw.text(
                "...",
                [x0 + held.len() as f32 * STEP + 2.0, y + 6.0],
                0xFFAA_AAAA,
                true,
            );
        }
    }
}

pub(crate) fn register(r: &mut dyn Registrar) {
    for kind in ["item", "fluid"] {
        for size in ["1k", "4k", "16k", "64k", "256k"] {
            let k = r.item_key(&format!("ae2:{kind}_storage_cell_{size}"));
            r.item_part(k, CellIcons);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn largest_first_and_more() {
        let keys: Vec<AeKey> = ["a", "b", "c"]
            .iter()
            .map(|i| AeKey::item(&format!("minecraft:{i}"), "", 0, 0))
            .collect();
        let (cards, held, more) = shown(
            &keys,
            &[5, 50, 0],
            vec!["ae2:void_card".into()],
            1,
            true,
            true,
        );
        assert_eq!(cards.len(), 1);
        assert_eq!(held.len(), 1);
        assert_eq!(held[0].1, 50);
        assert!(more);
        let (cards, held, more) = shown(
            &keys,
            &[5, 50, 0],
            vec!["ae2:void_card".into()],
            5,
            false,
            false,
        );
        assert!(cards.is_empty() && held.is_empty() && !more);
    }

    #[test]
    fn component_size() {
        assert_eq!(size(0, 0, false), None);
        assert_eq!(size(0, 3, true), Some([72.0, 20.0]));
        assert_eq!(size(2, 1, false), Some([36.0, 38.0]));
    }
}
