//! AE2's inscriber (InscriberBlockEntity, InscriberRecipes, InscriberMenu, InscriberScreen, InscriberTESR).
//!
//! Slots: 0 top press, 1 middle, 2 bottom press, 3 output, 4..8 acceleration cards. A recipe the slots make runs
//! `MAX_STEPS` steps (2, 3, 5, 10 or 50 a tick by cards, 10 AE a step from its own 1600 AE buffer or the network);
//! then the press shuts for 16 ticks: at the eighth the middle (and, pressing, both presses) is used up and the result
//! joins the output. A Name Press names the middle item. Settings: Auto-Export, separate sides (automation reaches the
//! top press from above, the bottom one from below, the middle and output from the sides) and the input stack size
//! (4 or 64).

use crate::cells::*;
use crate::machines::{Access, BACK, INSCRIBER, TEXT, bar_place, base_screen, card_slot, cards, inside, slot_icon};
use crate::screens::{side_panel, spr, state_button, tr};
use crate::util::*;
use mod_api::*;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

pub(crate) const SLOTS: usize = 8;
pub(crate) const OUTPUT: usize = 3;
const CARDS_FROM: usize = 4;
const CARD_SLOTS: usize = 4;
/// InscriberBlockEntity.MAX_PROCESSING_STEPS.
pub(crate) const MAX_STEPS: i32 = 200;
/// The AE a step of progress costs (`powerConsumption = 10 * speedFactor`).
const AE_PER_STEP: f64 = 10.0;
/// AEBasePoweredBlockEntity's internal buffer (`setInternalMaxPower(1600)`).
const INTERNAL_MAX: f64 = 1600.0;
/// Its FE face (the game's buffer, emptied into the AE buffer each tick): 1600 AE at the default 2 AE per FE.
pub(crate) const FE_BUFFER: i32 = 800;
const NAME_PRESS: &str = "ae2:name_press";

/// The toolbar's buttons (menu button ids).
pub(crate) const BTN_EXPORT: i32 = 5;
pub(crate) const BTN_SIDES: i32 = 6;
pub(crate) const BTN_BUFFER: i32 = 7;
const TOOLBAR: [i32; 3] = [BTN_SIDES, BTN_EXPORT, BTN_BUFFER];

/// The speed factor (steps a tick) by acceleration cards installed.
pub(crate) fn speed_factor(cards: usize) -> i32 {
    [2, 3, 5, 10, 50][cards.min(4)]
}

// ---- Recipes

/// An `ae2:inscriber` recipe: the items each slot takes (None: it must be empty), whether it uses up the presses
/// (`press` mode) and what it makes.
#[derive(Clone, Debug, Default)]
pub(crate) struct Recipe {
    pub(crate) top: Option<HashSet<String>>,
    pub(crate) middle: HashSet<String>,
    pub(crate) bottom: Option<HashSet<String>>,
    pub(crate) press: bool,
    pub(crate) result: String,
    pub(crate) count: u32,
}

fn parse(host: &dyn Host) -> Vec<Recipe> {
    host.recipes_of_type("ae2:inscriber")
        .iter()
        .filter_map(|r| {
            let json = &r.json;
            let ingredients = json.get("ingredients")?;
            let ing = |slot: &str| ingredients.get(slot).map(|v| crate::world::ingredient_ids(host, v));
            let result = json.get("result")?;
            Some(Recipe {
                top: ing("top"),
                middle: ing("middle")?,
                bottom: ing("bottom"),
                press: json.get("mode").and_then(|m| m.as_str()) == Some("press"),
                result: result.get("id").and_then(|v| v.as_str())?.to_owned(),
                count: result.get("count").and_then(|v| v.as_u64()).map_or(1, |n| n.clamp(1, 64) as u32),
            })
        })
        .collect()
}

/// The recipes, read again every ten seconds (InscriberRecipes is asked each tick and by every hopper).
static RECIPES: Mutex<Option<(std::time::Instant, Arc<Vec<Recipe>>)>> = Mutex::new(None);

pub(crate) fn recipes(host: &dyn Host) -> Arc<Vec<Recipe>> {
    let mut g = RECIPES.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, r)) = g.as_ref()
        && at.elapsed().as_secs() < 10
    {
        return r.clone();
    }
    let fresh = Arc::new(parse(host));
    // A host without them (a client) keeps no empty list for the server.
    if !fresh.is_empty() {
        *g = Some((std::time::Instant::now(), fresh.clone()));
    } else if let Some((_, r)) = g.as_ref() {
        return r.clone();
    }
    fresh
}

pub(crate) fn forget_all() {
    *RECIPES.lock().unwrap_or_else(|e| e.into_inner()) = None;
    PENDING.lock().unwrap_or_else(|e| e.into_inner()).clear();
}

fn fits(want: &Option<HashSet<String>>, have: Option<&str>) -> bool {
    match (want, have) {
        (None, None) => true,
        (Some(set), Some(item)) => set.contains(item),
        _ => false,
    }
}

/// InscriberRecipes.findRecipe: the recipe the slots make; the two presses go either way up.
pub(crate) fn find<'a>(all: &'a [Recipe], top: Option<&str>, middle: &str, bottom: Option<&str>) -> Option<&'a Recipe> {
    all.iter().find(|r| r.middle.contains(middle) && (fits(&r.top, top) && fits(&r.bottom, bottom) || fits(&r.top, bottom) && fits(&r.bottom, top)))
}

/// InscriberBlockEntity.BaseFilter.allowInsert: whether `item` may go in `slot` (0 top, 1 middle, 2 bottom) next to
/// what the others hold: a Name Press always in a press slot, anything in the middle beside a Name Press, otherwise
/// only what some recipe could still use. No recipes known (a client): yes, the server decides.
pub(crate) fn can_insert(all: &[Recipe], slot: usize, item: &str, held: [Option<&str>; 3]) -> bool {
    if slot > 2 {
        return false;
    }
    if all.is_empty() {
        return true;
    }
    if slot != 1 && item == NAME_PRESS {
        return true;
    }
    if slot == 1 && (held[0] == Some(NAME_PRESS) || held[2] == Some(NAME_PRESS)) {
        return true;
    }
    let mut s = held;
    s[slot] = Some(item);
    let (top, middle, bottom) = (s[0], s[1], s[2]);
    let has = |set: &Option<HashSet<String>>, what: &str| set.as_ref().is_some_and(|s| s.contains(what));
    all.iter().any(|r| {
        middle.is_none_or(|m| r.middle.contains(m))
            && match (top, bottom) {
                (None, None) => true,
                (Some(p), None) | (None, Some(p)) => has(&r.top, p) || has(&r.bottom, p),
                (Some(a), Some(b)) => has(&r.top, a) && has(&r.bottom, b) || has(&r.bottom, a) && has(&r.top, b),
            }
    })
}

/// What a press says it prints (NamePressItem's `InscribeName`, or a name an anvil gave it).
fn press_name(stack: &ItemStack) -> String {
    if let Some(Tag::String(n)) = custom_data(stack).and_then(|d| d.get(&java("InscribeName"))) {
        return n.to_string_lossy();
    }
    match stack.components().and_then(|c| c.as_nbt().get(&java("minecraft:custom_name")).cloned()) {
        Some(Tag::String(s)) => text_of(&s.to_string_lossy()),
        Some(Tag::Compound(c)) => match c.get(&java("text")) {
            Some(Tag::String(t)) => t.to_string_lossy(),
            _ => String::new(),
        },
        _ => String::new(),
    }
}

/// A name as plain text: a JSON text component's `text`, or the string itself.
pub(crate) fn text_of(s: &str) -> String {
    match mod_api::serde_json::from_str::<mod_api::serde_json::Value>(s) {
        Ok(mod_api::serde_json::Value::String(t)) => t,
        Ok(v) => v.get("text").and_then(|t| t.as_str()).map_or_else(|| s.to_owned(), str::to_owned),
        Err(_) => s.to_owned(),
    }
}

/// The Name Press's join of the two presses' names (InscriberRecipes.makeNamePressRecipe).
pub(crate) fn joined_name(top: &str, bottom: &str) -> String {
    [top, bottom].into_iter().filter(|n| !n.is_empty()).collect::<Vec<_>>().join(" ")
}

fn named(stack: &ItemStack, name: &str) -> Option<ItemStack> {
    let mut out = with_count(stack, 1)?;
    if !name.is_empty() {
        let mut patch = out.components().map(|c| c.as_nbt().clone()).unwrap_or_default();
        patch.insert(java("minecraft:custom_name"), Tag::String(java(name)));
        let _ = out.set_components(patch);
    }
    Some(out)
}

/// What the slots make now, and whether it uses up the presses.
fn task(host: &dyn Host, all: &[Recipe], items: &[Option<ItemStack>]) -> Option<(ItemStack, bool)> {
    let middle = items.get(1)?.as_ref()?;
    let (a, b) = (items[0].as_ref(), items[2].as_ref());
    let is_name = |s: Option<&ItemStack>| s.is_some_and(|p| sid(p) == NAME_PRESS);
    if is_name(a) && (is_name(b) || b.is_none()) || a.is_none() && is_name(b) {
        let name = joined_name(&a.map(press_name).unwrap_or_default(), &b.map(press_name).unwrap_or_default());
        return Some((named(middle, &name)?, false));
    }
    let r = find(all, a.map(sid), sid(middle), b.map(sid))?;
    Some((new_stack(host, &r.result, r.count)?, r.press))
}

/// Whether the output slot takes `result` whole.
fn room(host: &dyn Host, result: &ItemStack, out: &Option<ItemStack>) -> bool {
    match out {
        None => true,
        Some(o) => o.is_stackable_with(result) && o.count() + result.count() <= max_stack(host, o),
    }
}

// ---- The tick

/// Inscribers whose output waits for Auto-Export (done after the level's block entities, through ITEMS).
static PENDING: Mutex<Vec<(&'static str, Pos)>> = Mutex::new(Vec::new());

/// The FE that came in, into the AE buffer, as far as it has room.
fn fe_into_buffer(fields: &mut Fields) {
    let held = match fields.nbt.get(&java("energy")) {
        Some(Tag::Int(n)) if *n > 0 => *n,
        _ => return,
    };
    let ratio = crate::net::ae_per_fe();
    let stored = crate::net::internal_power(&fields.nbt);
    let fe = f64::from(held).min(((INTERNAL_MAX - stored).max(0.0) / ratio).floor()) as i32;
    if fe > 0 {
        crate::net::set_internal_power(&mut fields.nbt, stored + f64::from(fe) * ratio);
        fields.nbt.insert(java("energy"), Tag::Int(held - fe));
    }
}

/// InscriberBlockEntity.tickingRequest.
pub(crate) fn tick(host: &mut dyn ServerHost, pos: Pos, fields: &mut Fields) {
    if fields.items.len() < SLOTS {
        fields.items.resize(SLOTS, None);
    }
    if fields.data.len() < 3 {
        fields.data.resize(3, 0);
    }
    let dim = host.dimension();
    fe_into_buffer(fields);
    let flag = |k: &str| i32::from(nbt_i64(&fields.nbt, k) != 0);
    fields.data[1] = MAX_STEPS;
    fields.data[2] = flag("auto_export") | flag("separate_sides") << 1 | flag("small_buffer") << 2;
    if flag("auto_export") != 0 && fields.items[OUTPUT].is_some() {
        let mut p = PENDING.lock().unwrap_or_else(|e| e.into_inner());
        if !p.contains(&(dim, pos)) {
            p.push((dim, pos));
        }
    }
    let all = recipes(&*host);
    let job = task(&*host, &all, &fields.items);
    let mut progress = (nbt_i64(&fields.nbt, "processingTime") as i32).clamp(0, MAX_STEPS);
    let smash = nbt_i64(&fields.nbt, "smash");
    if smash > 0 {
        // The press is shut: halfway the item is made; sixteen ticks on it opens again.
        set_nbt_i64(&mut fields.nbt, "smash", smash + 1);
        if smash == 8 {
            if let Some((result, press)) = job.filter(|(r, _)| room(&*host, r, &fields.items[OUTPUT])) {
                let used: &[usize] = if press { &[0, 1, 2] } else { &[1] };
                for &slot in used {
                    if let Some(s) = fields.items[slot].take() {
                        fields.items[slot] = with_count(&s, s.count() - 1);
                    }
                }
                let out = match fields.items[OUTPUT].take() {
                    Some(o) => with_count(&o, o.count() + result.count()),
                    None => Some(result),
                };
                fields.items[OUTPUT] = out;
            }
            progress = 0;
        } else if smash >= 16 {
            set_nbt_i64(&mut fields.nbt, "smash", 0);
        }
    } else if job.as_ref().is_some_and(|(r, _)| room(&*host, r, &fields.items[OUTPUT])) {
        // hasCraftWork: progress by the cards, paid from the buffer if it has it all, else from the network.
        let speed = speed_factor(cards(&fields.items, CARDS_FROM, CARD_SLOTS, "ae2:speed_card"));
        let mult = crate::config::config().power_usage_multiplier;
        let cost = AE_PER_STEP * f64::from(speed) * mult;
        let stored = crate::net::internal_power(&fields.nbt);
        let paid = if stored > cost - 0.01 {
            crate::net::set_internal_power(&mut fields.nbt, (stored - cost).max(0.0));
            true
        } else if crate::net::device_state(dim, pos).0.is_some() {
            let got = crate::net::extract_power(dim, pos, cost);
            if got < cost - 0.01 {
                // Not enough: what came goes back (extractAEPower SIMULATE first, in AE2).
                crate::net::inject_power(dim, pos, got);
                false
            } else {
                true
            }
        } else {
            false
        };
        if paid {
            progress += speed;
            if progress > MAX_STEPS {
                progress = MAX_STEPS;
                set_nbt_i64(&mut fields.nbt, "smash", 1);
                set_nbt_i64(&mut fields.nbt, "smash_at", crate::net::with_net(dim, |n| n.tick) as i64);
            }
        }
    } else {
        progress = 0;
    }
    if nbt_i64(&fields.nbt, "processingTime") != i64::from(progress) {
        set_nbt_i64(&mut fields.nbt, "processingTime", i64::from(progress));
    }
    fields.data[0] = progress;
    // What the window shows (InscriberBlockEntity.writeToStream): the four items, and when the press shut.
    let look = look_of(fields);
    let old = match fields.nbt.get(&java("look")) {
        Some(Tag::String(s)) => s.to_string_lossy(),
        _ => String::new(),
    };
    if look != old {
        fields.nbt.insert(java("look"), Tag::String(java(&look)));
        crate::storage::with_store(|s| s.syncs.insert((dim, pos)));
    }
}

fn look_of(fields: &Fields) -> String {
    let mut parts: Vec<String> = (0..4).map(|i| fields.items.get(i).cloned().flatten().map_or(String::new(), |s| sid(&s).to_owned())).collect();
    let smash = nbt_i64(&fields.nbt, "smash") != 0;
    parts.push(if smash { nbt_i64(&fields.nbt, "smash_at").to_string() } else { "0".to_owned() });
    parts.join(",")
}

/// Auto-Export (InscriberBlockEntity's `pushOutResult`): the output into what is beside it (not above or below with
/// separate sides: those faces reach the presses), each move one transaction with the output's own slot.
pub(crate) fn export_pending(host: &mut dyn ServerHost, level: LevelRef) {
    let dim = host.dimension();
    let mine: Vec<Pos> = {
        let mut p = PENDING.lock().unwrap_or_else(|e| e.into_inner());
        let (here, rest): (Vec<_>, Vec<_>) = p.drain(..).partition(|(d, _)| *d == dim);
        *p = rest;
        here.into_iter().map(|(_, pos)| pos).collect()
    };
    for pos in mine {
        if crate::net::block_id(&*host, level, pos) != INSCRIBER {
            continue;
        }
        let Some(be) = host.block_entity(level, pos) else { continue };
        let nbt = be.data(&*host).unwrap_or_default();
        if nbt_i64(&nbt, "auto_export") == 0 {
            continue;
        }
        let separate = nbt_i64(&nbt, "separate_sides") != 0;
        for d in SIDES {
            if separate && d[1] != 0 {
                continue;
            }
            let Some(out) = be.item_slot(&*host, OUTPUT as u32).ok().flatten() else { break };
            let target = add(pos, d);
            let Some(h) = level.capability(host, &ITEMS, target, Some(crate::parts::dir_of(crate::parts::neg(d)))) else { continue };
            let res = ItemResource::of(&out);
            let n = out.count();
            let _ = host.transaction(|host, tx| {
                let went = h.insert(host, tx, &res, n).unwrap_or(0).min(n);
                if went == 0 {
                    return Err(());
                }
                be.set_item_slot(host, tx, OUTPUT as u32, with_count(&out, n - went)).map_err(|_| ())?;
                Ok(())
            });
        }
    }
}

// ---- Automation and the menu's rules

/// The most of `stack` slot `slot` may hold when automation puts it in (0: none): AutomationFilter (nothing while the
/// press is shut, nothing into the output) and BaseFilter, to the input stack size.
pub(crate) fn insert_limit(host: &dyn Host, at: &Access<'_>, slot: usize, stack: &ItemStack) -> u32 {
    if slot > 2 || nbt_i64(at.nbt, "smash") != 0 {
        return 0;
    }
    let held = [0, 1, 2].map(|i| at.items.get(i).and_then(Option::as_ref).map(sid));
    if !can_insert(&recipes(host), slot, sid(stack), held) {
        return 0;
    }
    if nbt_i64(at.nbt, "small_buffer") != 0 { 4 } else { 64 }
}

/// AutomationFilter.allowExtract: never while the press is shut; the output always, the presses only with separate
/// sides (each from its own face), the middle never.
pub(crate) fn may_extract(nbt: &Compound, slot: usize) -> bool {
    if nbt_i64(nbt, "smash") != 0 {
        return false;
    }
    slot == OUTPUT || nbt_i64(nbt, "separate_sides") != 0 && (slot == 0 || slot == 2)
}

/// ITEMS for the inscriber (getExposedInventoryForSide).
pub(crate) fn items(host: &mut dyn ServerHost, at: BlockSide) -> Option<Box<dyn ResourceHandler<ItemResource>>> {
    let be = host.block_entity(at.level, at.pos)?;
    let separate = be.data(&*host).is_some_and(|n| nbt_i64(&n, "separate_sides") != 0);
    let slots = match (separate, at.side) {
        (true, Some(Direction::Up)) => vec![0],
        (true, Some(Direction::Down)) => vec![2],
        (true, Some(_)) => vec![1, OUTPUT],
        _ => vec![0, 1, 2, OUTPUT],
    };
    crate::machines::handler(host, &at, INSCRIBER, slots)
}

/// InscriberMenu's slots: the presses and middle by BaseFilter (to the input stack size), the output takes nothing,
/// the cards one speed card a slot.
pub(crate) fn slot_rules(host: &dyn Host, context: &MenuSlotContext, slot: usize, stack: &ItemStack, may_place: &mut bool, max_stack: &mut u32) {
    let shown = |i: usize| context.slot(MenuContainer::Main, i);
    let pos = context.pos;
    match slot {
        0..=2 => {
            let shown_items = [0, 1, 2].map(shown);
            let held = [0, 1, 2].map(|i| shown_items[i].as_ref().map(sid));
            *may_place = can_insert(&recipes(host), slot, sid(stack), held);
            let small = pos
                .and_then(|p| Some(host.block_entity(here(host)?, p)?.data(host)?))
                .is_some_and(|n| nbt_i64(&n, "small_buffer") != 0);
            if small {
                *max_stack = (*max_stack).min(4);
            }
        }
        OUTPUT => *may_place = false,
        _ => {
            *may_place = sid(stack) == "ae2:speed_card";
            *max_stack = 1;
        }
    }
}

/// A toolbar button (the server's side): its setting flipped.
pub(crate) fn button(fields: &mut Fields, id: i32, back: bool) {
    let key = match id {
        BTN_EXPORT => "auto_export",
        BTN_SIDES => "separate_sides",
        BTN_BUFFER => "small_buffer",
        _ => return,
    };
    crate::machines::cycle(&mut fields.nbt, key, 2, back);
}

// ---- The screen (inscriber.json over inscriber.png)

pub(crate) fn screen() -> MenuScreenLayout {
    let mut slots = vec![[45.0, 16.0], [63.0, 39.0], [45.0, 62.0], [113.0, 40.0]];
    slots.extend((0..CARD_SLOTS).map(card_slot));
    base_screen(176.0, 176.0, slots, 94.0)
}

/// The toolbar's icons (states.png) for the flags in data slot 2.
fn icons(bits: i32) -> [[f32; 2]; 3] {
    [
        if bits & 2 != 0 { [80.0, 96.0] } else { [96.0, 96.0] },
        if bits & 1 != 0 { [128.0, 96.0] } else { [112.0, 96.0] },
        if bits & 4 != 0 { [144.0, 96.0] } else { [160.0, 96.0] },
    ]
}

pub(crate) fn draw(host: &dyn ClientHost, data: &[i32], d: &mut dyn DrawContext, o: [f32; 2]) {
    spr(d, "ae2:gui/inscriber", o, [0.0, 0.0], [0.0, 0.0, 176.0, 176.0]);
    side_panel(d, o, 176.0, CARD_SLOTS);
    d.text(&tr(host, "gui.ae2.Inscriber", "Inscriber"), [o[0] + 8.0, o[1] + 6.0], TEXT, false);
    d.text(&tr(host, "container.inventory", "Inventory"), [o[0] + 8.0, o[1] + 83.0], TEXT, false);
    // RestrictedInputSlot's INSCRIBER_PLATE and INSCRIBER_INPUT icons, the cards' UPGRADES.
    slot_icon(d, o, [45.0, 16.0], [224.0, 32.0]);
    slot_icon(d, o, [63.0, 39.0], [224.0, 48.0]);
    slot_icon(d, o, [45.0, 62.0], [224.0, 32.0]);
    for i in 0..CARD_SLOTS {
        slot_icon(d, o, card_slot(i), crate::machines::UPGRADE_ICON);
    }
    let bits = data.get(2).copied().unwrap_or(0);
    for (i, icon) in icons(bits).into_iter().enumerate() {
        state_button(d, o, bar_place(i), icon);
    }
    // The progress bar (6 by 18 at 135, 39) fills upwards.
    let (done, total) = (data.first().copied().unwrap_or(0), data.get(1).copied().unwrap_or(MAX_STEPS).max(1));
    let h = (18.0 * done.clamp(0, total) as f32 / total as f32).round();
    if h > 0.0 {
        d.sprite("ae2:gui/inscriber", [135.0, 177.0 + 18.0 - h, 6.0, h], [o[0] + 135.0, o[1] + 39.0 + 18.0 - h, 6.0, h], None);
    }
}

pub(crate) fn click(at: [f32; 2]) -> Option<i32> {
    TOOLBAR.iter().enumerate().find(|(i, _)| {
        let p = bar_place(*i);
        inside(at, [p[0], p[1], 16.0, 16.0])
    }).map(|(_, id)| *id)
}

pub(crate) fn tooltip(host: &dyn ClientHost, data: &[i32], at: [f32; 2], lines: &mut Vec<String>) {
    let bits = data.get(2).copied().unwrap_or(0);
    let t = |k: &str, f: &str| tr(host, k, f);
    if let Some(id) = click(at) {
        let (title, value) = match id {
            BTN_SIDES => (
                t("gui.tooltips.ae2.InscriberSideness", "Automation Access Mode"),
                if bits & 2 != 0 {
                    t("gui.tooltips.ae2.InscriberSidenessSeparate", "Automation can access slots from specific sides. Top and bottom slots can be extracted from.")
                } else {
                    t("gui.tooltips.ae2.InscriberSidenessCombined", "Automation can access any slot from any side. Input slots cannot be extracted from.")
                },
            ),
            BTN_EXPORT => (
                t("gui.tooltips.ae2.AutoExport", "Auto-Export"),
                if bits & 1 != 0 {
                    t("gui.tooltips.ae2.AutoExportOn", "Produced items will be auto-exported through any extraction side.")
                } else {
                    t("gui.tooltips.ae2.AutoExportOff", "Produced items will be not be auto-exported.")
                },
            ),
            _ => (
                t("gui.tooltips.ae2.InscriberBufferSize", "Input Slot Stack Size"),
                if bits & 4 != 0 { t("gui.tooltips.ae2.InscriberBufferLow", "Stack to 4") } else { t("gui.tooltips.ae2.InscriberBufferHigh", "Stack to 64") },
            ),
        };
        lines.push(title);
        lines.push(format!("§7{value}"));
        return;
    }
    if inside(at, [135.0, 39.0, 6.0, 18.0]) {
        let (done, total) = (data.first().copied().unwrap_or(0), data.get(1).copied().unwrap_or(MAX_STEPS).max(1));
        lines.push(format!("{}%", done.clamp(0, total) * 100 / total));
    }
}

// ---- The press in the window (InscriberTESR)

/// When each shut press was first seen (by position and the server tick it shut at), for its 0.8 second motion.
static PRESSES: Mutex<Option<HashMap<(Pos, String), std::time::Instant>>> = Mutex::new(None);

/// How far the heads have come in at `t` (0 open, 1 shut) of the press's motion in units of 0.4 s
/// (InscriberTESR's easeCompressMotion then easeDecompressMotion).
pub(crate) fn press_shut(t: f32) -> f32 {
    let t = t.clamp(0.0, 2.0);
    if t > 1.0 { 1.0 - (t - 1.0).powi(5) } else { 1.0 + 2.70158 * (t - 1.0).powi(3) + 1.70158 * (t - 1.0).powi(2) }
}

pub(crate) fn submit(host: &dyn ClientHost, event: &mut BlockEntitySubmitEvent<'_>) {
    let Some(Tag::String(look)) = event.entity.nbt.get(&java("look")) else { return };
    let look = look.to_string_lossy();
    let parts: Vec<&str> = look.split(',').collect();
    let item = |i: usize| parts.get(i).copied().filter(|s| !s.is_empty()).and_then(|id| new_stack(host, id, 1));
    let shut = match parts.get(4).filter(|s| **s != "0") {
        Some(at) => {
            let key = (event.pos, (*at).to_owned());
            let started = {
                let mut g = PRESSES.lock().unwrap_or_else(|e| e.into_inner());
                let m = g.get_or_insert_with(HashMap::new);
                m.retain(|_, t| t.elapsed().as_secs_f32() < 3.0);
                *m.entry(key).or_insert_with(std::time::Instant::now)
            };
            press_shut(started.elapsed().as_secs_f32() / 0.4)
        }
        None => 0.0,
    };
    let Some(level) = host.level(host.dimension()) else { return };
    let facing = level.block_property(host, event.pos, "facing").unwrap_or_else(|| "north".to_owned());
    let draw = &mut *event.draw;
    draw.push();
    let c = [0.5, 0.5, 0.5];
    match facing.as_str() {
        "up" => draw.rotate_about([1.0, 0.0, 0.0], 90.0, c),
        "down" => draw.rotate_about([1.0, 0.0, 0.0], -90.0, c),
        _ => {}
    }
    let lift = 2.5 * shut;
    let face = || Some(CubeFace::new("ae2:block/inscriber_inside"));
    // The heads: the top one hangs from the frame, the bottom one sits on it.
    draw.cube([3.0, 11.0 - lift, 3.0], [13.0, 12.0 - lift, 13.0], [face(), face(), face(), face(), face(), face()], 0xFF_FFFF);
    draw.cube([3.0, 4.0 + lift, 3.0], [13.0, 5.0 + lift, 13.0], [face(), face(), face(), face(), face(), face()], 0xFF_FFFF);
    let mut flat = |stack: ItemStack, y: f32, flip: bool| {
        draw.push();
        draw.translate([0.5, y / 16.0, 0.5]);
        draw.rotate_about([1.0, 0.0, 0.0], if flip { -90.0 } else { 90.0 }, [0.0, 0.0, 0.0]);
        draw.scale([0.4, 0.4, 0.4]);
        draw.item(&stack, ItemDisplayContext::Fixed, None, 0);
        draw.pop();
    };
    if let Some(top) = item(0) {
        flat(top, 10.9 - lift, true);
    }
    if let Some(bottom) = item(2) {
        flat(bottom, 5.1 + lift, false);
    }
    if let Some(middle) = item(1).or_else(|| item(3)) {
        flat(middle, 8.0, false);
    }
    draw.pop();
}

/// The menu button ids this screen sends, for the server's check.
pub(crate) fn is_button(id: i32) -> bool {
    TOOLBAR.contains(&(id & !BACK))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(items: &[&str]) -> HashSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn book() -> Vec<Recipe> {
        vec![
            // Printing: the press stays.
            Recipe { top: Some(set(&["ae2:silicon_press"])), middle: set(&["ae2:silicon"]), bottom: None, press: false, result: "ae2:printed_silicon".into(), count: 1 },
            // Pressing: both presses used.
            Recipe {
                top: Some(set(&["ae2:printed_logic_processor"])),
                middle: set(&["minecraft:redstone"]),
                bottom: Some(set(&["ae2:printed_silicon"])),
                press: true,
                result: "ae2:logic_processor".into(),
                count: 1,
            },
            // Middle only.
            Recipe { top: None, middle: set(&["ae2:certus_quartz_crystal"]), bottom: None, press: false, result: "ae2:certus_quartz_dust".into(), count: 1 },
        ]
    }

    #[test]
    fn finds_recipes_either_way_up() {
        let b = book();
        assert_eq!(find(&b, Some("ae2:silicon_press"), "ae2:silicon", None).map(|r| r.result.as_str()), Some("ae2:printed_silicon"));
        // The press below works as well as above (supportsRecipeFlipping).
        assert_eq!(find(&b, None, "ae2:silicon", Some("ae2:silicon_press")).map(|r| r.result.as_str()), Some("ae2:printed_silicon"));
        let p = find(&b, Some("ae2:printed_silicon"), "minecraft:redstone", Some("ae2:printed_logic_processor")).unwrap();
        assert!(p.press);
        assert_eq!(p.result, "ae2:logic_processor");
        // A missing press, or an extra one, makes nothing.
        assert!(find(&b, Some("ae2:printed_silicon"), "minecraft:redstone", None).is_none());
        assert!(find(&b, Some("ae2:silicon_press"), "ae2:certus_quartz_crystal", None).is_none());
        assert_eq!(find(&b, None, "ae2:certus_quartz_crystal", None).map(|r| r.result.as_str()), Some("ae2:certus_quartz_dust"));
        assert!(find(&b, None, "minecraft:dirt", None).is_none());
    }

    #[test]
    fn base_filter_takes_only_what_a_recipe_could_use() {
        let b = book();
        // A press goes in either press slot; not in the middle.
        assert!(can_insert(&b, 0, "ae2:silicon_press", [None, None, None]));
        assert!(can_insert(&b, 2, "ae2:silicon_press", [None, None, None]));
        assert!(!can_insert(&b, 0, "minecraft:dirt", [None, None, None]));
        assert!(can_insert(&b, 1, "ae2:silicon", [None, None, None]));
        assert!(!can_insert(&b, 1, "minecraft:dirt", [None, None, None]));
        // Next to a silicon press, only silicon fits the middle.
        assert!(can_insert(&b, 1, "ae2:silicon", [Some("ae2:silicon_press"), None, None]));
        assert!(!can_insert(&b, 1, "minecraft:redstone", [Some("ae2:silicon_press"), None, None]));
        // The second press must pair with the first.
        assert!(can_insert(&b, 2, "ae2:printed_silicon", [Some("ae2:printed_logic_processor"), None, None]));
        assert!(!can_insert(&b, 2, "ae2:silicon_press", [Some("ae2:printed_logic_processor"), None, None]));
        // The output and the cards take nothing.
        assert!(!can_insert(&b, OUTPUT, "ae2:silicon", [None, None, None]));
        // The Name Press: in a press slot always; with one there, anything in the middle.
        assert!(can_insert(&b, 0, NAME_PRESS, [None, None, None]));
        assert!(!can_insert(&b, 1, NAME_PRESS, [None, None, None]));
        assert!(can_insert(&b, 1, "minecraft:dirt", [Some(NAME_PRESS), None, None]));
        assert!(can_insert(&b, 1, "minecraft:dirt", [None, None, Some(NAME_PRESS)]));
        // No recipes known (a client): the server decides.
        assert!(can_insert(&[], 1, "minecraft:dirt", [None, None, None]));
    }

    #[test]
    fn speed_cards_and_press_motion() {
        assert_eq!(speed_factor(0), 2);
        assert_eq!(speed_factor(4), 50);
        assert_eq!(speed_factor(9), 50);
        // 0 cards: 101 ticks of progress to pass 200 steps.
        assert_eq!((MAX_STEPS / speed_factor(0)) + 1, 101);
        assert!(press_shut(0.0).abs() < 1e-3);
        assert!((press_shut(1.0) - 1.0).abs() < 1e-6);
        assert!(press_shut(2.0).abs() < 1e-6);
    }

    #[test]
    fn name_press_names() {
        assert_eq!(joined_name("Hello", "World"), "Hello World");
        assert_eq!(joined_name("", "World"), "World");
        assert_eq!(joined_name("", ""), "");
        assert_eq!(text_of("{\"text\":\"Bob\"}"), "Bob");
        assert_eq!(text_of("\"Bob\""), "Bob");
        assert_eq!(text_of("Bob"), "Bob");
    }

    #[test]
    fn automation_access() {
        let mut nbt = Compound::new();
        // Combined: only the output comes out.
        assert!(may_extract(&nbt, OUTPUT));
        assert!(!may_extract(&nbt, 0) && !may_extract(&nbt, 1) && !may_extract(&nbt, 2));
        set_nbt_i64(&mut nbt, "separate_sides", 1);
        assert!(may_extract(&nbt, 0) && may_extract(&nbt, 2) && !may_extract(&nbt, 1));
        // Nothing while the press is shut.
        set_nbt_i64(&mut nbt, "smash", 3);
        assert!(!may_extract(&nbt, OUTPUT));
        assert!(is_button(BTN_SIDES | BACK) && !is_button(99));
    }
}
