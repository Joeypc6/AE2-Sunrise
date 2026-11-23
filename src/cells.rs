//! Storage cells (AE2's `BasicStorageCell`, `BasicCellInventory`): what a cell holds rides on the cell item itself, in
//! its `minecraft:custom_data` component, so it survives breaking the drive, dropping the cell or trading it.
//!
//! The layout is the upstream port's (`ae2-horizon/mixins/ae2/me.rs`, "Cells"): `ae2:cell` holds the keys (`k`, one
//! string each) and amounts (`a`, a long array); `ae2:cell_id`/`ae2:cell_ver` name the cell and the version of its
//! contents (a drive keeps a mounted cell while its id and version are the ones it wrote); `ae2:cell_status` is what
//! the drive's light shows. `ae2:config`, `ae2:upgrades` and `ae2:fuzzy` are the cell workbench's (partition, cards).
//! On the fork the item's components are read and written through `ItemStack::components`/`set_components`.

use crate::core::{AeKey, BasicCellInventory, CellContents, CellState, CellType, CellUpgrades, FuzzyMode, KeyType, StorageTier};
use crate::util::*;
use mod_api::*;
use std::collections::HashMap;
use std::sync::OnceLock;

pub(crate) const CUSTOM_DATA: &str = "minecraft:custom_data";
pub(crate) const CELL_KEY: &str = "ae2:cell";

// ---- Items by id (keys stay for the process)

pub(crate) struct ItemIds {
    pub(crate) by_id: HashMap<String, ItemKey>,
    pub(crate) by_key: HashMap<ItemKey, String>,
}

static IDS: OnceLock<ItemIds> = OnceLock::new();

/// The item ids, read once from the host (any hook with a host calls this before the maps are needed).
pub(crate) fn ids(host: &dyn Host) -> &'static ItemIds {
    IDS.get_or_init(|| {
        let keys = host.item_keys();
        let by_id: HashMap<String, ItemKey> = keys.into_iter().filter_map(|k| Some((k.id(host)?.to_owned(), k))).collect();
        let by_key = by_id.iter().map(|(i, k)| (*k, i.clone())).collect();
        ItemIds { by_id, by_key }
    })
}

/// An item's id ("" before the maps are made: only `menu_slot_rules`, which has no host, can see that).
pub(crate) fn key_id(k: ItemKey) -> &'static str {
    IDS.get().and_then(|i| i.by_key.get(&k)).map_or("", String::as_str)
}

pub(crate) fn sid(s: &ItemStack) -> &'static str {
    key_id(s.item())
}

pub(crate) fn item_key(host: &dyn Host, id: &str) -> Option<ItemKey> {
    ids(host).by_id.get(id).copied()
}

pub(crate) fn new_stack(host: &dyn Host, id: &str, count: u32) -> Option<ItemStack> {
    ItemStack::new(item_key(host, id)?, count)
}

/// The item's stack limit (`getMaxStackSize`).
pub(crate) fn max_stack(host: &dyn Host, stack: &ItemStack) -> u32 {
    host.max_stack_size(stack.item()).unwrap_or(64).max(1)
}

/// `stack` with `count` (none for 0).
pub(crate) fn with_count(stack: &ItemStack, count: u32) -> Option<ItemStack> {
    if count == 0 {
        return None;
    }
    let mut out = stack.clone();
    out.set_count(count).ok()?;
    Some(out)
}

// ---- Keys and stacks

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

/// The stack's component patch, damage included (so a fresh tool and one repaired to 0 stay two keys, as two
/// ItemStacks are), as network NBT in hex; "" for a plain stack.
fn components_hex(stack: &ItemStack) -> String {
    let mut patch = stack.components().map(|c| c.as_nbt().clone()).unwrap_or_default();
    if let Some(d) = stack.damage_component() {
        patch.insert(java("minecraft:damage"), Tag::Int(d as i32));
    }
    if patch.is_empty() {
        return String::new();
    }
    Tag::Compound(patch).to_network_bytes().map(|b| hex(&b)).unwrap_or_default()
}

/// AE2's key of a stack (`AEItemKey.of`): its id, damage, maximum damage and other components.
pub(crate) fn key_of(host: &dyn Host, stack: &ItemStack) -> AeKey {
    let max = host.max_damage(stack.item()).unwrap_or(0) as i32;
    AeKey::item(sid(stack), &components_hex(stack), stack.damage() as i32, max)
}

/// `count` of a key's item (none for a fluid or an unknown item).
pub(crate) fn stack_of(host: &dyn Host, key: &AeKey, count: u32) -> Option<ItemStack> {
    if key.key_type() != KeyType::Items {
        return None;
    }
    let mut stack = new_stack(host, key.id(), count)?;
    if !key.components().is_empty()
        && let Some((Tag::Compound(patch), _)) = unhex(key.components()).and_then(|b| Tag::from_network_bytes(&b, None).ok())
    {
        let _ = stack.set_components(patch);
    }
    Some(stack)
}

/// A key as one string, saved on cells and sent to terminals: `id|damage|max|components` (a fluid `~id|0|0|comps`).
pub(crate) fn key_to_string(key: &AeKey) -> String {
    let mark = if key.key_type() == KeyType::Fluids { "~" } else { "" };
    format!("{mark}{}|{}|{}|{}", key.id(), key.fuzzy_search_value(), key.fuzzy_search_max_value(), key.components())
}

pub(crate) fn key_from_string(s: &str) -> Option<AeKey> {
    if let Some(fluid) = s.strip_prefix('~') {
        let mut parts = fluid.splitn(4, '|');
        let id = parts.next()?;
        return Some(AeKey::fluid(id, parts.nth(2).unwrap_or("")));
    }
    let mut parts = s.splitn(4, '|');
    let id = parts.next()?;
    let damage = parts.next()?.parse().ok()?;
    let max = parts.next()?.parse().ok()?;
    Some(AeKey::item(id, parts.next().unwrap_or(""), damage, max))
}

/// The bucket a fluid goes in and out of a terminal with (AE2 Fabric: the fluid's own bucket item).
pub(crate) fn fluid_bucket(fluid: &str) -> String {
    match fluid {
        "minecraft:water" => "minecraft:water_bucket".to_owned(),
        "minecraft:lava" => "minecraft:lava_bucket".to_owned(),
        f => format!("{f}_bucket"),
    }
}

/// The fluid a filled bucket holds.
pub(crate) fn bucket_fluid(item: &str) -> Option<String> {
    match item {
        "minecraft:water_bucket" => Some("minecraft:water".to_owned()),
        "minecraft:lava_bucket" => Some("minecraft:lava".to_owned()),
        "minecraft:bucket" | "minecraft:milk_bucket" | "minecraft:powder_snow_bucket" => None,
        i if i.ends_with("_bucket") && !i.starts_with("minecraft:") => Some(i.trim_end_matches("_bucket").to_owned()),
        _ => None,
    }
}

// ---- Cell types

/// `BasicStorageCell`'s types: the item and fluid cells, 1k to 256k.
pub(crate) fn cell_type(item: &str) -> Option<CellType> {
    // [tools] The matter cannon and colour applicator are cells too (`tool_cells`).
    if let Some(t) = crate::tool_cells::cell_type(item) {
        return Some(t);
    }
    let name = item.strip_prefix("ae2:")?;
    let (size, fluid) = match name.strip_prefix("item_storage_cell_") {
        Some(s) => (s, false),
        None => (name.strip_prefix("fluid_storage_cell_")?, true),
    };
    let tier = StorageTier::ALL.into_iter().find(|t| t.name_prefix == size)?;
    Some(if fluid { CellType::fluid_cell(tier) } else { CellType::item_cell(tier) })
}

/// Whether an item goes in a drive or a chest (`StorageCells.isCellHandled`): a storage cell or a creative cell.
pub(crate) fn is_cell_item(item: &str) -> bool {
    cell_type(item).is_some() || matches!(item, "ae2:creative_item_cell" | "ae2:creative_fluid_cell")
}

// ---- What the item carries

pub(crate) fn custom_data(stack: &ItemStack) -> Option<&Compound> {
    match stack.components()?.as_nbt().get(&java(CUSTOM_DATA))? {
        Tag::Compound(c) => Some(c),
        _ => None,
    }
}

/// `stack`'s custom data changed by `f` (an empty compound removes the component).
pub(crate) fn edit_custom_data(stack: &mut ItemStack, f: impl FnOnce(&mut Compound)) {
    let mut patch = stack.components().map(|c| c.as_nbt().clone()).unwrap_or_default();
    let mut data = match patch.remove(&java(CUSTOM_DATA)) {
        Some(Tag::Compound(c)) => c,
        _ => Compound::new(),
    };
    f(&mut data);
    if !data.is_empty() {
        patch.insert(java(CUSTOM_DATA), Tag::Compound(data));
    }
    let _ = stack.set_components(patch);
}

/// A component by its full id, its value its raw tag (the editor's descriptor for an override as saved).
struct RawComponent(String);

impl Component for RawComponent {
    type Value = Tag;
    fn id(&self) -> &str {
        &self.0
    }
    fn decode(&self, value: &Tag) -> Option<Tag> {
        Some(value.clone())
    }
    fn encode(&self, value: Tag) -> Tag {
        value
    }
}

/// An edit of a held stack, made on an owned copy as the port's code makes it: Sunrise main's `ItemPlace::edit` lends
/// a component editor, so the copy's changed overrides are staged on it (one no longer there inherits again). The item
/// and count stay as they are (a change to either is refused, as the copying edit refused it). False, nothing staged,
/// when the stack could not be copied or `f` changed its item or count.
pub(crate) fn stage_stack_edit(ed: &mut ItemComponentEditor<'_>, f: impl FnOnce(&mut ItemStack)) -> bool {
    let Some(before) = ed.original().project() else { return false };
    let mut after = before.clone();
    f(&mut after);
    if after.item() != before.item() || after.count() != before.count() {
        return false;
    }
    let old = before.components().map(|c| c.as_nbt().clone()).unwrap_or_default();
    let new = after.components().map(|c| c.as_nbt().clone()).unwrap_or_default();
    if old == new {
        return true;
    }
    for k in old.keys() {
        if !new.contains_key(k) {
            ed.remove(&RawComponent(k.to_string_lossy().trim_start_matches('!').to_owned()));
        }
    }
    let mut changed = Compound::new();
    for (k, v) in new.iter() {
        if old.get(k) != Some(v) {
            changed.insert(k.clone(), v.clone());
        }
    }
    ed.apply_components(&changed).is_ok()
}

/// `place`'s stack edited as an owned copy in `tx` (`stage_stack_edit`): Err, for the caller to roll back, when it
/// was not edited.
pub(crate) fn edit_stack(place: &ItemPlace, host: &mut dyn ServerHost, tx: &Transaction, f: impl FnOnce(&mut ItemStack)) -> Result<(), ()> {
    let mut ok = false;
    place.edit(host, tx, |ed| ok = stage_stack_edit(ed, f)).map_err(|_| ())?;
    if ok { Ok(()) } else { Err(()) }
}

/// A cell's id and version as written on it (none on a cell no network has stored in yet).
pub(crate) fn cell_id(stack: &ItemStack) -> Option<(i64, i64)> {
    let data = custom_data(stack)?;
    match (data.get(&java("ae2:cell_id")), data.get(&java("ae2:cell_ver"))) {
        (Some(Tag::Long(id)), Some(Tag::Long(ver))) => Some((*id, *ver)),
        _ => None,
    }
}

pub(crate) fn set_cell_id(stack: &mut ItemStack, id: i64, ver: i64, status: i64) {
    edit_custom_data(stack, |d| {
        d.insert(java("ae2:cell_id"), Tag::Long(id));
        d.insert(java("ae2:cell_ver"), Tag::Long(ver));
        d.insert(java("ae2:cell_status"), Tag::Long(status));
    });
}

pub(crate) fn read_cell(stack: &ItemStack) -> CellContents {
    let mut out = CellContents::default();
    let Some(Tag::Compound(cell)) = custom_data(stack).and_then(|d| d.get(&java(CELL_KEY))) else { return out };
    if let (Some(Tag::List(keys)), Some(Tag::LongArray(amts))) = (cell.get(&java("k")), cell.get(&java("a"))) {
        for (k, a) in keys.iter().zip(amts) {
            if let Tag::String(js) = k
                && let Some(key) = key_from_string(&js.to_string_lossy())
            {
                out.keys.push(key);
                out.amounts.push(*a);
                out.item_count += *a;
            }
        }
    }
    out
}

/// What a cell holds now, written on it.
pub(crate) fn write_cell(stack: &mut ItemStack, contents: &CellContents) {
    edit_custom_data(stack, |d| {
        if contents.keys.is_empty() {
            d.remove(&java(CELL_KEY));
        } else {
            let mut cell = Compound::new();
            cell.insert(java("k"), Tag::List(contents.keys.iter().map(|k| Tag::String(java(&key_to_string(k)))).collect()));
            cell.insert(java("a"), Tag::LongArray(contents.amounts.clone()));
            d.insert(java(CELL_KEY), Tag::Compound(cell));
        }
    });
}

/// The cell's partition (the cell workbench's config).
pub(crate) fn cell_config(stack: &ItemStack) -> Vec<AeKey> {
    match custom_data(stack).and_then(|d| d.get(&java("ae2:config"))) {
        Some(Tag::List(keys)) => keys
            .iter()
            .filter_map(|k| match k {
                Tag::String(s) => key_from_string(&s.to_string_lossy()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

pub(crate) fn cell_cards(stack: &ItemStack) -> Vec<String> {
    match custom_data(stack).and_then(|d| d.get(&java("ae2:upgrades"))) {
        Some(Tag::List(cards)) => cards.iter().filter_map(|t| if let Tag::String(s) = t { Some(s.to_string_lossy()) } else { None }).collect(),
        _ => Vec::new(),
    }
}

pub(crate) fn cell_upgrades(stack: &ItemStack) -> CellUpgrades {
    let cards = cell_cards(stack);
    let has = |c: &str| cards.iter().any(|x| x == c);
    CellUpgrades {
        inverter: has("ae2:inverter_card"),
        fuzzy: has("ae2:fuzzy_card"),
        equal_distribution: has("ae2:equal_distribution_card"),
        void: has("ae2:void_card"),
    }
}

pub(crate) fn cell_fuzzy(stack: &ItemStack) -> FuzzyMode {
    match custom_data(stack).and_then(|d| d.get(&java("ae2:fuzzy"))) {
        Some(Tag::String(s)) => FuzzyMode::from_saved(&s.to_string_lossy()),
        _ => FuzzyMode::IgnoreAll,
    }
}

/// The cell's inventory as saved on it.
pub(crate) fn cell_inventory(stack: &ItemStack, cell: CellType) -> BasicCellInventory {
    let mut inv = BasicCellInventory::new(cell, &cell_config(stack), cell_fuzzy(stack), cell_upgrades(stack), &read_cell(stack));
    // StorageCells.isCellHandled / canFitInsideCell: a storage cell with something in it doesn't go in another.
    // [tools] The matter cannon and colour applicator keep out what they can't use (`isBlackListed`).
    let tool = sid(stack);
    if crate::tool_cells::cell_type(tool).is_some() {
        inv.accepts = Some(Box::new(move |k: &AeKey| k.key_type() == KeyType::Items && crate::tool_cells::accepts(tool, k.id()) && !is_filled_cell_key(k)));
        return inv;
    }
    inv.accepts = Some(Box::new(|k: &AeKey| !is_filled_cell_key(k)));
    inv
}

/// A key that is a storage cell with something in it (AE2 keeps those out of cells).
fn is_filled_cell_key(key: &AeKey) -> bool {
    key.key_type() == KeyType::Items && cell_type(key.id()).is_some() && key.components().len() > 0 && {
        // The contents are in the components: a cell with `ae2:cell` in them holds something.
        unhex(key.components())
            .and_then(|b| Tag::from_network_bytes(&b, None).ok())
            .is_some_and(|(t, _)| match t {
                Tag::Compound(p) => matches!(p.get(&java(CUSTOM_DATA)), Some(Tag::Compound(d)) if d.contains_key(&java(CELL_KEY))),
                _ => false,
            })
    }
}

pub(crate) fn status_number(state: CellState) -> i64 {
    match state {
        CellState::Absent => 0,
        CellState::Empty => 1,
        CellState::NotEmpty => 2,
        CellState::TypesFull => 3,
        CellState::Full => 4,
    }
}

/// The status a cell shows (drive light, item tint): saved on it when a network last wrote it, else worked out.
pub(crate) fn cell_status(stack: &ItemStack) -> i64 {
    let id = sid(stack);
    if matches!(id, "ae2:creative_item_cell" | "ae2:creative_fluid_cell") {
        return 1;
    }
    let Some(cell) = cell_type(id) else { return 0 };
    if let Some(Tag::Long(n)) = custom_data(stack).and_then(|d| d.get(&java("ae2:cell_status"))) {
        return *n;
    }
    status_number(cell_inventory(stack, cell).status())
}

/// CellState.getStateColor: the cell item's second tint and the drive's light.
pub(crate) const STATUS_COLORS: [u32; 5] = [0x000000, 0x00FF00, 0x00AAFF, 0xFFAA00, 0xFF0000];

/// The drive's look of a slot: `<model>:<status>` (the cell textures under `block/drive/cells/`), "" empty.
pub(crate) fn cell_look(stack: Option<&ItemStack>) -> String {
    let Some(stack) = stack else { return String::new() };
    let id = sid(stack);
    let name = id.strip_prefix("ae2:").unwrap_or("");
    let model = if let Some(t) = name.strip_prefix("item_storage_cell_") {
        format!("{t}_item_cell")
    } else if let Some(t) = name.strip_prefix("fluid_storage_cell_") {
        format!("{t}_fluid_cell")
    } else if name.starts_with("creative_") && name.ends_with("_cell") {
        "creative_cell".to_owned()
    } else {
        "default_cell".to_owned()
    };
    format!("{model}:{}", cell_status(stack))
}

/// `BasicStorageCell.getIdleDrain` of what sits in a drive or chest (a creative cell draws none).
pub(crate) fn idle_drain(stack: &ItemStack) -> f64 {
    cell_type(sid(stack)).map_or(0.0, |c| c.idle_drain)
}

/// `BasicStorageCell.appendHoverText` (StorageCellTooltip): bytes and types used, then the partition and cards.
pub(crate) fn cell_tooltip(stack: &ItemStack) -> Vec<String> {
    let Some(cell) = cell_type(sid(stack)) else { return Vec::new() };
    let inv = cell_inventory(stack, cell);
    let mut out = vec![
        format!("§7{} of {} Bytes Used", inv.used_bytes(), inv.total_bytes()),
        format!("§7{} of {} Types", inv.stored_item_types(), inv.total_item_types()),
    ];
    if !cell_config(stack).is_empty() {
        out.push(format!("§7{}", if cell_upgrades(stack).inverter { "Partitioned - Blacklisted" } else { "Partitioned - Whitelisted" }));
    }
    out
}
