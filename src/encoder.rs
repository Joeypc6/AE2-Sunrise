//! The pattern encoding terminal's encoding panel (AE2's PatternEncodingTermMenu, PatternEncodingLogic,
//! PatternEncodingTermScreen and its four EncodingModePanels), server and client.
//!
//! - **Cells.** The encoder keeps AE2's two config inventories, 81 inputs and 27 outputs, in the block entity's
//!   `ghosts` string as 108 cells. They are saved in the upstream port's order, which keeps what the twelve-cell
//!   encoder saved: the first nine inputs, the first three outputs, then the other 72 inputs and the other 24
//!   outputs (`input_cell`, `output_cell`). Crafting uses inputs 0..9, smithing 0..3, stonecutting 0.
//! - **Modes.** Crafting (substitutions, fluid substitutions, the grid's result), processing (81 inputs in rows of
//!   three and 27 outputs with a scroll bar, cycle outputs, the amount dialog on a middle click), smithing (its
//!   result, substitutions) and stonecutting (the stonecutter's results to pick from, four a row, scrolled).
//! - **Encoding** (PatternEncodingTermMenu.encode): the mode's pattern into the encoded slot, from a blank pattern
//!   (one used up) or over the encoded pattern there; when nothing valid is entered, an encoded pattern there turns
//!   back into a blank one (clearPattern). The item's data is what `autocraft::decode` reads (`ae2:pattern`: `in`,
//!   `out`, `rem`, `grid`, `subs`, `fluids`), condensed as AEProcessingPattern has it, plus `sparse` (the cells, for
//!   loading back), `recipe` (a stonecutting pattern's recipe) and the substitution flags.
//! - **Loading** (PatternEncodingLogic.loadEncodedPattern): an encoded pattern put in the encoded slot fills the
//!   cells and the mode it was made in.
//!
//! Every click is checked on the server (`autocraft::menu_button`: the player's open terminal, in range, rate
//! limited); amounts are clamped to AE2's 999 999 units; the amount dialog's value comes as a message and is
//! applied in the encoder's own tick.

use crate::autocraft::{ENC, ENC_CELLS, ENC_CLEAR, ENC_ENCODE, ENC_FLUID_SUBS, ENC_GHOST, ENC_MODE, ENC_SUBS, is_pattern, pattern_stack_subs};
use crate::cells::*;
use crate::core::{AeKey, AmountFormat, KeyType};
use crate::util::*;
use mod_api::*;
use std::sync::Mutex;

/// AE2's processing inventories.
pub(crate) const INPUTS: usize = 81;
pub(crate) const OUTPUTS: usize = 27;
/// Cycle the processing outputs (cycleProcessingOutput).
pub(crate) const ENC_CYCLE: i32 = ENC | 5;
/// `ENC_STONECUT + n`: the stonecutter's `n`th result picked (setStonecuttingRecipeId).
pub(crate) const ENC_STONECUT: i32 = ENC | (2 << 12);
/// The ghost cell clicks' range (`ENC_GHOST | cell << 3 | action`).
const GHOST_SPAN: i32 = 1 << 12;
/// SetProcessingPatternAmountScreen.getMaxAmount: 999 999 units.
const MAX_UNITS: i64 = 999_999;
const PATTERN_KEY: &str = "ae2:pattern";

pub(crate) type Cell = Option<(AeKey, i64)>;

// ---- Cells (pure)

/// The saved cell of processing input `k` (0..81).
pub(crate) fn input_cell(k: usize) -> usize {
    if k < 9 { k } else { k + 3 }
}

/// The saved cell of processing output `j` (0..27).
pub(crate) fn output_cell(j: usize) -> usize {
    if j < 3 { 9 + j } else { 84 + j - 3 }
}

/// The 81 inputs, in order.
pub(crate) fn inputs_of(cells: &[Cell]) -> Vec<Cell> {
    (0..INPUTS).map(|k| cells.get(input_cell(k)).cloned().flatten()).collect()
}

/// The 27 outputs, in order (the first is the primary output).
pub(crate) fn outputs_of(cells: &[Cell]) -> Vec<Cell> {
    (0..OUTPUTS).map(|j| cells.get(output_cell(j)).cloned().flatten()).collect()
}

/// The cells a mode takes clicks on (saved cell indexes).
pub(crate) fn mode_takes(mode: i64, cell: usize) -> bool {
    match mode {
        0 => cell < 9,
        1 => cell < ENC_CELLS,
        2 => cell < 3,
        _ => cell == 0,
    }
}

/// cycleProcessingOutput: each set output becomes the next set one after it (the primary output moves on).
pub(crate) fn cycle<T: Clone>(outputs: &[Option<T>]) -> Vec<Option<T>> {
    let n = outputs.len();
    (0..n)
        .map(|i| {
            outputs[i].as_ref()?;
            (1..n).map(|j| &outputs[(i + j) % n]).find(|o| o.is_some()).cloned().flatten()
        })
        .collect()
}

/// AEPatternHelper.condenseStacks: the same key's amounts added up, in the order each first appears.
pub(crate) fn condense(list: impl IntoIterator<Item = (AeKey, i64)>) -> Vec<(AeKey, i64)> {
    let mut out: Vec<(AeKey, i64)> = Vec::new();
    for (k, n) in list {
        match out.iter_mut().find(|(o, _)| *o == k) {
            Some((_, total)) => *total = total.saturating_add(n),
            None => out.push((k, n)),
        }
    }
    out
}

/// PatternEncodingLogic.fixCraftingRecipes: outside processing, every input is one item (no fluids).
pub(crate) fn fix_crafting(cells: &mut [Cell]) {
    for k in 0..INPUTS {
        let Some(c) = cells.get_mut(input_cell(k)) else { continue };
        match c {
            Some((key, _)) if key.key_type() != KeyType::Items => *c = None,
            Some((_, n)) => *n = 1,
            None => {}
        }
    }
}

/// The amount dialog's value for `key`: none (the cell emptied) at 0 or less, else at most 999 999 units.
pub(crate) fn amount_for(key: &AeKey, amount: i64) -> Option<i64> {
    let most = MAX_UNITS * key.key_type().amount_per_unit().max(1);
    (amount > 0).then(|| amount.min(most))
}

/// A processing pattern can be encoded: some input and the primary output set.
pub(crate) fn processing_ready(inputs: &[Cell], outputs: &[Cell]) -> bool {
    inputs.iter().any(Option::is_some) && outputs.first().is_some_and(Option::is_some)
}

/// RecipeManager.getRecipesFor's order (by the result's description id: its namespace, then its path).
pub(crate) fn stonecut_sort(list: &mut [(String, AeKey, i64)]) {
    let order = |k: &AeKey| {
        let (ns, path) = k.id().split_once(':').unwrap_or(("minecraft", k.id()));
        (ns.to_owned(), path.to_owned())
    };
    list.sort_by(|a, b| order(&a.1).cmp(&order(&b.1)).then_with(|| a.0.cmp(&b.0)));
}

/// What a ghost click does to a cell (FakeSlot): an item on the cursor puts a copy there (one outside processing,
/// the stack's count in processing; right-click adds one of the same, or one); a fluid container right-clicked in
/// processing puts its fluid, a bucket's worth; an empty cursor clears it (right-click takes one away in processing).
/// `wheel`: up (true) or down a unit.
pub(crate) fn ghost_change(current: Cell, carried: Option<(AeKey, i64, Option<AeKey>)>, right: bool, processing: bool) -> Cell {
    match carried {
        Some((key, count, fluid)) => {
            if processing
                && right
                && let Some(f) = fluid
            {
                let unit = f.key_type().amount_per_unit();
                return Some((f, unit));
            }
            if !processing {
                return Some((key, 1));
            }
            match current {
                Some((k, n)) if right && k == key => Some((k, amount_for(&key, n + 1).unwrap_or(n))),
                _ if right => Some((key, 1)),
                _ => Some((key.clone(), amount_for(&key, count.max(1)).unwrap_or(1))),
            }
        }
        None => match current {
            Some((k, n)) if right && processing && n > k.key_type().amount_per_unit() => {
                let step = k.key_type().amount_per_unit();
                Some((k, n - step))
            }
            _ => None,
        },
    }
}

// ---- The block entity's state

fn mode_of(nbt: &Compound) -> i64 {
    nbt_i64(nbt, "mode").clamp(0, 3)
}

fn subs_on(nbt: &Compound) -> bool {
    nbt_i64(nbt, "subs") != 0
}

/// Fluid substitution: on unless turned off (PatternEncodingLogic's default).
fn fluid_subs_on(nbt: &Compound) -> bool {
    nbt_i64(nbt, "nofsubs") == 0
}

fn nbt_string(nbt: &Compound, key: &str) -> Option<String> {
    match nbt.get(&java(key)) {
        Some(Tag::String(s)) => Some(s.to_string_lossy()).filter(|s| !s.is_empty()),
        _ => None,
    }
}

fn set_nbt_string(nbt: &mut Compound, key: &str, value: Option<&str>) {
    match value {
        Some(v) => {
            nbt.insert(java(key), Tag::String(java(v)));
        }
        None => {
            nbt.remove(&java(key));
        }
    }
}

pub(crate) fn cells_of(nbt: &Compound) -> Vec<Cell> {
    crate::parts::ghosts(nbt, ENC_CELLS)
}

// ---- Recipes

fn item_stack(host: &dyn Host, cell: &Cell) -> Option<ItemStack> {
    cell.as_ref().filter(|(k, _)| k.key_type() == KeyType::Items).and_then(|(k, _)| stack_of(host, k, 1))
}

/// The game's recipe for the cells in `mode` (0 crafting, 2 smithing).
fn recipe_of(host: &dyn Host, level: LevelRef, mode: i64, cells: &[Cell]) -> Option<RecipeMatch> {
    let st = |i: usize| item_stack(host, cells.get(i).unwrap_or(&None));
    let (kind, input) = match mode {
        0 => {
            // getEncodedCraftingIngredient: a fluid in the grid makes no recipe.
            if cells.iter().take(9).flatten().any(|(k, _)| k.key_type() != KeyType::Items) {
                return None;
            }
            ("minecraft:crafting", RecipeInput::Crafting { width: 3, height: 3, slots: (0..9).map(st).collect() })
        }
        // encodeSmithingTablePattern: all three set.
        2 => ("minecraft:smithing", RecipeInput::Smithing { template: Some(st(0)?), base: Some(st(1)?), addition: Some(st(2)?) }),
        _ => return None,
    };
    let kind = Identifier::parse(kind).ok()?;
    level.recipe_for(host, &kind, &input).ok().flatten()
}

/// The stonecutter's recipes for an input item: (recipe id, result, count), in the menu's order.
pub(crate) fn stonecut_options(host: &dyn Host, input: &Cell) -> Vec<(String, AeKey, i64)> {
    let Some((k, _)) = input.as_ref().filter(|(k, _)| k.key_type() == KeyType::Items) else { return Vec::new() };
    let id = k.id();
    let mut out = Vec::new();
    for r in host.recipes_of_type("minecraft:stonecutting").iter() {
        let (ingredient, result) = match &r.rule {
            Some(RecipeRule::Stonecutting { input, result }) => (input, result.clone()),
            _ => match (r.ingredients.first(), r.results.first()) {
                (Some(i), Some(res)) => (i, res.clone()),
                _ => continue,
            },
        };
        if ingredient.items.contains(id) || ingredient.order.iter().any(|x| x == id) {
            out.push((r.id.clone(), key_of(host, &with_count(&result, 1).unwrap_or(result.clone())), i64::from(result.count().max(1))));
        }
    }
    stonecut_sort(&mut out);
    out
}

fn stonecut_ingredient(host: &dyn Host, recipe: &str) -> Vec<String> {
    host.recipes_of_type("minecraft:stonecutting")
        .iter()
        .find(|r| r.id == recipe)
        .and_then(|r| match &r.rule {
            Some(RecipeRule::Stonecutting { input, .. }) => Some(input.order.clone()),
            _ => r.ingredients.first().map(|i| i.order.clone()),
        })
        .unwrap_or_default()
}

/// The smithing recipe's three ingredients' items (template, base, addition), for substitutions.
fn smithing_ingredients(host: &dyn Host, recipe: &str) -> [Vec<String>; 3] {
    let list = host.recipes_of_type("minecraft:smithing");
    let Some(r) = list.iter().find(|r| r.id == recipe) else { return Default::default() };
    let order = |i: Option<&Ingredient>| i.map(|i| i.order.clone()).unwrap_or_default();
    match &r.rule {
        Some(RecipeRule::SmithingTransform { template, base, addition, .. }) => [order(template.as_ref()), order(Some(base)), order(addition.as_ref())],
        Some(RecipeRule::SmithingTrim { template, base, addition, .. }) => [order(Some(template)), order(Some(base)), order(Some(addition))],
        _ => Default::default(),
    }
}

/// The picked stonecutting recipe, if it is still one of the input's (updateStonecuttingRecipes forgets it otherwise).
fn stonecut_pick(nbt: &Compound, options: &[(String, AeKey, i64)]) -> Option<usize> {
    let id = nbt_string(nbt, "stonecut")?;
    options.iter().position(|(r, _, _)| *r == id)
}

// ---- Server: the screen's state

/// The encoder's state for its viewer (`ae2:enc`): `m <mode> <subs> <fluid subs> <picked result or -1>`, a line
/// `c <cell> <amount> <key>` per set cell (`+` after it: the network can craft it), `r <amount> <key>` for the
/// crafting grid's or smithing table's result, `o <amount> <key>` for each stonecutting result, `f <cell>` for the
/// crafting cells fluid substitution would take a fluid for.
pub(crate) fn view(host: &mut dyn ServerHost, level: LevelRef, dim: &'static str, pos: Pos) -> String {
    let nbt = crate::bus::nbt_at(&*host, level, pos).unwrap_or_default();
    let mode = mode_of(&nbt);
    let cells = cells_of(&nbt);
    let craftable: std::collections::HashSet<AeKey> = crate::autocraft::craftables(dim, pos).0.into_iter().collect();
    let options = if mode == 3 { stonecut_options(&*host, &cells[0]) } else { Vec::new() };
    let pick = stonecut_pick(&nbt, &options).map_or(-1, |i| i as i64);
    let mut out = format!("m {mode} {} {} {pick}\n", u8::from(subs_on(&nbt)), u8::from(fluid_subs_on(&nbt)));
    for (i, c) in cells.iter().enumerate() {
        if let Some((k, n)) = c {
            let input = i < 9 || (12..84).contains(&i);
            let plus = if input && craftable.contains(k) { " +" } else { "" };
            out.push_str(&format!("c {i} {n} {}{plus}\n", key_to_string(k)));
        }
    }
    if (mode == 0 || mode == 2)
        && let Some(m) = recipe_of(&*host, level, mode, &cells)
    {
        out.push_str(&format!("r {} {}\n", m.output.count(), key_to_string(&key_of(&*host, &m.output))));
    }
    if mode == 0 {
        for (i, c) in cells.iter().take(9).enumerate() {
            if c.as_ref().is_some_and(|(k, _)| bucket_fluid(k.id()).is_some()) {
                out.push_str(&format!("f {i}\n"));
            }
        }
    }
    for (_, k, n) in options.iter().take(256) {
        out.push_str(&format!("o {n} {}\n", key_to_string(k)));
    }
    out
}

// ---- Server: buttons

/// The amount dialog's values waiting for the encoder's tick: (player, cell, amount) by encoder.
static AMOUNTS: Mutex<Vec<(u128, Pos, usize, i64)>> = Mutex::new(Vec::new());

/// `encamt <cell> <amount>` from the amount dialog (SET_FILTER on a processing slot): kept for the encoder's tick.
pub(crate) fn amount_message(host: &dyn Host, player: u128, cell: Option<usize>, amount: Option<i64>) {
    let (Some(cell), Some(amount)) = (cell, amount) else { return };
    if cell >= ENC_CELLS {
        return;
    }
    let Some((pos, block, _)) = crate::storage::viewer_for_button(host, player) else { return };
    if block != "ae2:pattern_encoding_terminal" {
        return;
    }
    let mut list = AMOUNTS.lock().unwrap_or_else(|e| e.into_inner());
    list.retain(|(p, _, _, _)| *p != player);
    if list.len() < 256 {
        list.push((player, pos, cell, amount));
    }
}

/// A button of the encoder's panel (the caller checked the player's terminal and the rate).
pub(crate) fn button(host: &mut dyn ServerHost, id: i32, fields: &mut Fields, carried: &Option<ItemStack>) {
    let mode = mode_of(&fields.nbt);
    let mut cells = cells_of(&fields.nbt);
    match id {
        _ if (ENC_MODE..ENC_MODE + 4).contains(&id) => {
            let next = i64::from(id - ENC_MODE);
            set_nbt_i64(&mut fields.nbt, "mode", next);
            if next != 1 {
                fix_crafting(&mut cells);
            }
        }
        ENC_CLEAR => cells = vec![None; ENC_CELLS],
        ENC_SUBS => {
            let on = !subs_on(&fields.nbt);
            set_nbt_i64(&mut fields.nbt, "subs", i64::from(on));
            return;
        }
        ENC_FLUID_SUBS => {
            let on = !fluid_subs_on(&fields.nbt);
            set_nbt_i64(&mut fields.nbt, "nofsubs", i64::from(!on));
            fields.nbt.remove(&java("fsubs"));
            return;
        }
        ENC_ENCODE => {
            encode(host, fields);
            return;
        }
        ENC_CYCLE => {
            // canCycleProcessingOutputs: processing, with two outputs set at least.
            if mode != 1 || outputs_of(&cells).iter().filter(|c| c.is_some()).count() < 2 {
                return;
            }
            let next = cycle(&outputs_of(&cells));
            for (j, o) in next.into_iter().enumerate() {
                cells[output_cell(j)] = o;
            }
        }
        _ if (ENC_STONECUT..ENC_STONECUT + 4096).contains(&id) => {
            if mode != 3 {
                return;
            }
            let options = stonecut_options(&*host, &cells[0]);
            let Some((recipe, _, _)) = options.get((id - ENC_STONECUT) as usize) else { return };
            set_nbt_string(&mut fields.nbt, "stonecut", Some(recipe));
            return;
        }
        _ if (ENC_GHOST..ENC_GHOST + GHOST_SPAN).contains(&id) => {
            let cell = ((id - ENC_GHOST) >> 3) as usize;
            let action = (id - ENC_GHOST) & 7;
            if !mode_takes(mode, cell) {
                return;
            }
            let processing = mode == 1;
            match action {
                // The wheel in processing: a unit more or less.
                2 | 3 if processing => {
                    if let Some((k, a)) = cells[cell].as_mut() {
                        let step = k.key_type().amount_per_unit();
                        *a = if action == 2 { amount_for(k, *a + step).unwrap_or(*a) } else { (*a - step).max(step) };
                    }
                }
                0 | 1 => {
                    let held = carried.as_ref().map(|st| {
                        let fluid = bucket_fluid(sid(st)).map(|f| AeKey::fluid(&f, ""));
                        (key_of(&*host, &with_count(st, 1).unwrap_or(st.clone())), i64::from(st.count()), fluid)
                    });
                    cells[cell] = ghost_change(cells[cell].take(), held, action == 1, processing);
                }
                _ => return,
            }
        }
        _ => return,
    }
    crate::parts::set_ghosts(&mut fields.nbt, &cells);
}

/// The encoder's tick: the amount dialog's values, and an encoded pattern put in its slot loaded into the cells.
pub(crate) fn tick(host: &mut dyn ServerHost, pos: Pos, fields: &mut Fields) {
    let mine: Vec<(u128, usize, i64)> = {
        let mut list = AMOUNTS.lock().unwrap_or_else(|e| e.into_inner());
        if list.is_empty() {
            Vec::new()
        } else {
            let (mine, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut *list).into_iter().partition(|(_, p, _, _)| *p == pos);
            *list = rest;
            mine.into_iter().map(|(p, _, c, a)| (p, c, a)).collect()
        }
    };
    if !mine.is_empty() && mode_of(&fields.nbt) == 1 {
        let mut cells = cells_of(&fields.nbt);
        for (player, cell, amount) in mine {
            // Still this player's open encoder.
            if crate::storage::viewer_for_button(&*host, player).is_none_or(|(p, _, _)| p != pos) {
                continue;
            }
            if let Some((k, _)) = cells[cell].clone() {
                cells[cell] = amount_for(&k, amount).map(|n| (k, n));
            }
        }
        crate::parts::set_ghosts(&mut fields.nbt, &cells);
    }
    // onChangeInventory(encodedPatternInv): load what was put there.
    let held = fields.items.get(2).cloned().flatten().filter(|s| is_pattern(sid(s)));
    let mark = held.as_ref().map_or(0, |s| hash_of(&key_to_string(&key_of(&*host, s))) as i64);
    let seen = fields.nbt.contains_key(&java("loaded"));
    if !seen || nbt_i64(&fields.nbt, "loaded") != mark {
        set_nbt_i64(&mut fields.nbt, "loaded", mark);
        // An encoder from before this (no mark yet) keeps its cells: only a pattern put in from now on loads.
        if seen && let Some(st) = held {
            load(&st, fields);
        }
    }
}

fn hash_of(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish() | 1
}

// ---- Server: encoding

fn pattern_data(stack: &ItemStack) -> Option<Compound> {
    match custom_data(stack)?.get(&java(PATTERN_KEY))? {
        Tag::Compound(p) => Some(p.clone()),
        _ => None,
    }
}

fn strings(p: &Compound, key: &str) -> Vec<String> {
    match p.get(&java(key)) {
        Some(Tag::List(l)) => l.iter().map(|t| if let Tag::String(s) = t { s.to_string_lossy() } else { String::new() }).collect(),
        Some(Tag::String(s)) => s.to_string_lossy().split(',').map(str::to_owned).collect(),
        _ => Vec::new(),
    }
}

/// `n*key` entries with their cell: `cell*n*key`.
fn sparse_entries(list: &[(usize, AeKey, i64)]) -> Tag {
    Tag::List(list.iter().map(|(i, k, n)| Tag::String(java(&format!("{i}*{n}*{}", key_to_string(k))))).collect())
}

fn read_sparse(p: &Compound, key: &str) -> Vec<(usize, AeKey, i64)> {
    strings(p, key)
        .iter()
        .filter_map(|s| {
            let (i, rest) = s.split_once('*')?;
            let (n, k) = rest.split_once('*')?;
            Some((i.parse().ok()?, key_from_string(k)?, n.parse().ok().filter(|n: &i64| *n > 0)?))
        })
        .collect()
}

/// PatternEncodingLogic.loadEncodedPattern: the pattern's mode and cells (and its substitution flags, its
/// stonecutting recipe).
pub(crate) fn load(stack: &ItemStack, fields: &mut Fields) {
    let Some((inputs, outputs, _)) = crate::autocraft::pattern_lists(stack) else { return };
    let Some(p) = pattern_data(stack) else { return };
    let mut cells: Vec<Cell> = vec![None; ENC_CELLS];
    let flag = |key: &str| matches!(p.get(&java(key)), Some(Tag::Byte(1)));
    let mode = match sid(stack) {
        "ae2:processing_pattern" => {
            let sparse_in = read_sparse(&p, "sparse_in");
            let sparse_out = read_sparse(&p, "sparse_out");
            if sparse_in.is_empty() {
                for (k, (key, n)) in inputs.into_iter().enumerate().take(INPUTS) {
                    cells[input_cell(k)] = Some((key, n));
                }
            } else {
                for (k, key, n) in sparse_in.into_iter().filter(|(k, _, _)| *k < INPUTS) {
                    cells[input_cell(k)] = Some((key, n));
                }
            }
            if sparse_out.is_empty() {
                for (j, (key, n)) in outputs.into_iter().enumerate().take(OUTPUTS) {
                    cells[output_cell(j)] = Some((key, n));
                }
            } else {
                for (j, key, n) in sparse_out.into_iter().filter(|(j, _, _)| *j < OUTPUTS) {
                    cells[output_cell(j)] = Some((key, n));
                }
            }
            1
        }
        "ae2:crafting_pattern" => {
            let grid: Vec<usize> = strings(&p, "grid").iter().filter_map(|x| x.parse().ok()).collect();
            for (i, (key, _)) in inputs.into_iter().enumerate() {
                let at = grid.get(i).copied().unwrap_or(i);
                if at < 9 {
                    cells[at] = Some((key, 1));
                }
            }
            let subs = flag("subst") || strings(&p, "subs").iter().any(|s| !s.is_empty());
            set_nbt_i64(&mut fields.nbt, "subs", i64::from(subs));
            // A pattern from before the flag was written: fluid substitution as it had fluids.
            let fluids = flag("fsubst") || strings(&p, "fluids").iter().any(|s| !s.is_empty()) || !p.contains_key(&java("fsubst_set"));
            set_nbt_i64(&mut fields.nbt, "nofsubs", i64::from(!fluids));
            0
        }
        "ae2:smithing_table_pattern" => {
            for (i, (key, _)) in inputs.into_iter().enumerate().take(3) {
                cells[i] = Some((key, 1));
            }
            set_nbt_i64(&mut fields.nbt, "subs", i64::from(flag("subst")));
            2
        }
        _ => {
            if let Some((key, _)) = inputs.into_iter().next() {
                cells[0] = Some((key, 1));
            }
            let recipe = nbt_string(&p, "recipe");
            set_nbt_string(&mut fields.nbt, "stonecut", recipe.as_deref());
            set_nbt_i64(&mut fields.nbt, "subs", i64::from(flag("subst")));
            3
        }
    };
    set_nbt_i64(&mut fields.nbt, "mode", mode);
    crate::parts::set_ghosts(&mut fields.nbt, &cells);
}

fn edit_pattern(st: &mut ItemStack, f: impl FnOnce(&mut Compound)) {
    edit_custom_data(st, |d| {
        if let Some(Tag::Compound(p)) = d.get_mut(&java(PATTERN_KEY)) {
            f(p);
        }
    });
}

/// The mode's pattern made of the cells (encodePattern), or None when they make none.
fn make_pattern(host: &dyn Host, level: LevelRef, nbt: &Compound) -> Option<ItemStack> {
    let mode = mode_of(nbt);
    let cells = cells_of(nbt);
    let subs = subs_on(nbt);
    let one = |k: &AeKey| (k.clone(), 1i64);
    match mode {
        1 => {
            let inputs = inputs_of(&cells);
            let outputs = outputs_of(&cells);
            if !processing_ready(&inputs, &outputs) {
                return None;
            }
            let sparse = |list: &[Cell]| list.iter().enumerate().filter_map(|(i, c)| c.clone().map(|(k, n)| (i, k, n))).collect::<Vec<_>>();
            let (sin, sout) = (sparse(&inputs), sparse(&outputs));
            let ins = condense(sin.iter().map(|(_, k, n)| (k.clone(), *n)));
            let outs = condense(sout.iter().map(|(_, k, n)| (k.clone(), *n)));
            let mut st = pattern_stack_subs(host, "ae2:processing_pattern", &ins, &outs, &[], &[], &[])?;
            edit_pattern(&mut st, |p| {
                p.insert(java("sparse_in"), sparse_entries(&sin));
                p.insert(java("sparse_out"), sparse_entries(&sout));
            });
            Some(st)
        }
        0 => {
            let m = recipe_of(host, level, 0, &cells)?;
            let (mut inputs, mut left, mut at) = (Vec::new(), Vec::new(), Vec::new());
            for i in 0..9 {
                if let Some((k, _)) = &cells[i] {
                    at.push(i.to_string());
                    inputs.push(one(k));
                    left.push(m.remainders.get(i).cloned().flatten().map(|s| key_of(host, &s)));
                }
            }
            if inputs.is_empty() {
                return None;
            }
            let out = (key_of(host, &with_count(&m.output, 1)?), i64::from(m.output.count()));
            // AECraftingPattern: with substitutions, each slot takes what its ingredient takes; with fluid
            // substitution, a bucket's slot its fluid.
            let want_fluids = fluid_subs_on(nbt);
            let id = m.recipe.to_string();
            let ingredients: Vec<Vec<String>> = ["minecraft:crafting_shaped", "minecraft:crafting_shapeless", "minecraft:crafting"]
                .iter()
                .flat_map(|kind| {
                    host.recipes_of_type(kind).iter().filter(|r| r.id == id).flat_map(|r| r.ingredients.iter().map(|i| i.order.clone())).collect::<Vec<_>>()
                })
                .collect();
            let (mut alts, mut fluids) = (Vec::new(), Vec::new());
            for (k, _) in &inputs {
                let found = ingredients.iter().find(|i| i.iter().any(|x| x == k.id())).cloned().unwrap_or_default();
                alts.push(if subs { found.into_iter().filter(|a| a != k.id()).collect::<Vec<_>>().join("|") } else { String::new() });
                fluids.push(if want_fluids { bucket_fluid(k.id()).unwrap_or_default() } else { String::new() });
            }
            let mut st = pattern_stack_subs(host, "ae2:crafting_pattern", &inputs, &[out], &left, &alts, &fluids)?;
            edit_pattern(&mut st, |p| {
                p.insert(java("grid"), Tag::String(java(&at.join(","))));
                p.insert(java("subst"), Tag::Byte(i8::from(subs)));
                p.insert(java("fsubst"), Tag::Byte(i8::from(want_fluids)));
                p.insert(java("fsubst_set"), Tag::Byte(1));
            });
            Some(st)
        }
        2 => {
            let m = recipe_of(host, level, 2, &cells)?;
            let inputs: Vec<(AeKey, i64)> = cells.iter().take(3).map(|c| c.as_ref().map(|(k, _)| one(k))).collect::<Option<_>>()?;
            let out = (key_of(host, &with_count(&m.output, 1)?), i64::from(m.output.count()));
            let alts: Vec<String> = if subs {
                let ing = smithing_ingredients(host, &m.recipe.to_string());
                inputs.iter().zip(ing).map(|((k, _), list)| list.into_iter().filter(|a| a != k.id()).collect::<Vec<_>>().join("|")).collect()
            } else {
                Vec::new()
            };
            let mut st = pattern_stack_subs(host, "ae2:smithing_table_pattern", &inputs, &[out], &[], &alts, &[])?;
            edit_pattern(&mut st, |p| {
                p.insert(java("grid"), Tag::String(java("0,1,2")));
                p.insert(java("subst"), Tag::Byte(i8::from(subs)));
            });
            Some(st)
        }
        _ => {
            // encodeStonecuttingPattern: the picked recipe, for the input set.
            let options = stonecut_options(host, &cells[0]);
            let (recipe, out, n) = options.get(stonecut_pick(nbt, &options)?)?.clone();
            let (k, _) = cells[0].clone()?;
            let alts: Vec<String> =
                if subs { vec![stonecut_ingredient(host, &recipe).into_iter().filter(|a| a != k.id()).collect::<Vec<_>>().join("|")] } else { Vec::new() };
            let mut st = pattern_stack_subs(host, "ae2:stonecutting_pattern", &[one(&k)], &[(out, n)], &[], &alts, &[])?;
            edit_pattern(&mut st, |p| {
                p.insert(java("grid"), Tag::String(java("0")));
                p.insert(java("recipe"), Tag::String(java(&recipe)));
                p.insert(java("subst"), Tag::Byte(i8::from(subs)));
            });
            Some(st)
        }
    }
}

/// PatternEncodingTermMenu.encode: the pattern into the encoded slot, from a blank one (used up) or over the encoded
/// one (or a blank one) there; nothing valid entered turns an encoded pattern there back into a blank one.
fn encode(host: &mut dyn ServerHost, fields: &mut Fields) {
    let Some(level) = here(&*host) else { return };
    if fields.items.len() < 3 {
        return;
    }
    let output = fields.items[2].clone();
    let Some(made) = make_pattern(&*host, level, &fields.nbt) else {
        // clearPattern.
        if let Some(o) = output.filter(|o| is_pattern(sid(o))) {
            fields.items[2] = new_stack(&*host, "ae2:blank_pattern", o.count());
        }
        return;
    };
    match output {
        Some(o) if !is_pattern(sid(&o)) && sid(&o) != "ae2:blank_pattern" => return,
        Some(_) => {}
        None => {
            let Some(b) = fields.items[1].clone().filter(|b| sid(b) == "ae2:blank_pattern") else { return };
            fields.items[1] = with_count(&b, b.count() - 1);
        }
    }
    // What was just written isn't loaded back (the cells already are it).
    set_nbt_i64(&mut fields.nbt, "loaded", hash_of(&key_to_string(&key_of(&*host, &made))) as i64);
    fields.items[2] = Some(made);
}

// ---- Client: the panel

/// The panel's corner: pattern.png's bottom part starts at the rows' foot; the panel's file positions are from 71
/// above it (`bottom` values in pattern_encoding_terminal.json and encoding/*.json: y = 249 - bottom).
const TABS: [[f32; 2]; 4] = [[173.0, 79.0], [173.0, 99.0], [173.0, 119.0], [173.0, 139.0]];
const ENCODE_AT: [f32; 2] = [147.0, 107.0];
const CLEAR_AT: [[f32; 2]; 2] = [[74.0, 86.0], [82.0, 86.0]];
const SUBS_AT: [f32; 2] = [84.0, 86.0];
const FLUID_SUBS_AT: [f32; 2] = [94.0, 86.0];
const CYCLE_AT: [f32; 2] = [127.0, 97.0];
/// Scrollbar.SMALL beside the processing inputs, and beside the stonecutter's results.
const PROC_BAR: [f32; 3] = [17.0, 93.0, 52.0];
const STONE_BAR: [f32; 3] = [119.0, 92.0, 54.0];
/// StonecuttingEncodingPanel: four results a row, three rows, 16 by 18 from 44, 8 in the panel (at 9, 85).
const STONE_AT: [f32; 2] = [53.0, 93.0];
const TEXT: u32 = 0xFF40_4040;
/// set_stock_amount.json's back tab.
const BACK_TAB: [f32; 4] = [154.0, 0.0, 22.0, 22.0];

#[derive(Clone)]
struct Dialog {
    cell: usize,
    key: AeKey,
    text: String,
}

#[derive(Default)]
struct View {
    mode: i64,
    subs: bool,
    fluid_subs: bool,
    cells: Vec<Cell>,
    craftable: Vec<bool>,
    fluid_cells: Vec<usize>,
    result: Option<(AeKey, i64)>,
    options: Vec<(AeKey, i64)>,
    pick: Option<usize>,
    proc_scroll: usize,
    stone_scroll: usize,
    /// The scroll bar held: 1 processing, 2 stonecutting.
    drag: u8,
    dialog: Option<Dialog>,
}

static VIEW: Mutex<Option<View>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut View) -> R) -> R {
    let mut g = VIEW.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(View::default))
}

fn inside(at: [f32; 2], r: [f32; 4]) -> bool {
    at[0] >= r[0] && at[0] < r[0] + r[2] && at[1] >= r[1] && at[1] < r[1] + r[3]
}

/// Where place `i` of the mode's grid is drawn: crafting's 3x3, processing's three visible input rows then its
/// three visible outputs, smithing's three, stonecutting's one.
fn place_at(i: usize, mode: i64) -> [f32; 2] {
    match mode {
        2 => [21.0 + 18.0 * i as f32, 111.0],
        3 => [21.0, 110.0],
        _ => {
            let left = if mode == 0 { 18.0 } else { 26.0 };
            if i < 9 { [left + 18.0 * (i % 3) as f32, 93.0 + 18.0 * (i / 3) as f32] } else { [110.0, 93.0 + 18.0 * (i - 9) as f32] }
        }
    }
}

fn places(mode: i64) -> usize {
    match mode {
        0 => 9,
        1 => 12,
        2 => 3,
        _ => 1,
    }
}

/// The saved cell place `i` shows, the processing panel scrolled `scroll` rows.
pub(crate) fn place_cell(i: usize, mode: i64, scroll: usize) -> usize {
    match (mode, i) {
        (1, 0..=8) => input_cell(scroll * 3 + i),
        (1, _) => output_cell(scroll + i - 9),
        _ => i,
    }
}

/// The processing panel's last scroll row (ProcessingEncodingPanel: 81 / 3 - 3).
const PROC_MOST: usize = INPUTS / 3 - 3;

fn stone_most(n: usize) -> usize {
    n.div_ceil(4).saturating_sub(3)
}

fn half_icon(d: &mut dyn DrawContext, b: [f32; 2], at: [f32; 2], icon: [f32; 2]) {
    d.sprite("ae2:gui/states", [240.0, 240.0, 16.0, 16.0], [b[0] + at[0], b[1] + at[1], 8.0, 8.0], None);
    d.sprite("ae2:gui/states", [icon[0], icon[1], 16.0, 16.0], [b[0] + at[0], b[1] + at[1], 8.0, 8.0], None);
}

fn scrollbar(d: &mut dyn DrawContext, b: [f32; 2], bar: [f32; 3], top: usize, most: usize) {
    let (src, dy) = if most == 0 { ([249.0, 0.0], 0.0) } else { ([242.0, 0.0], ((bar[2] - 15.0) * top as f32 / most as f32).floor()) };
    d.sprite("ae2:gui/pattern_modes", [src[0], src[1], 7.0, 15.0], [b[0] + bar[0], b[1] + bar[1] + dy, 7.0, 15.0], None);
}

fn bar_row(y: f32, bar: [f32; 3], most: usize) -> usize {
    if most == 0 {
        return 0;
    }
    let travel = (bar[2] - 15.0).max(1.0);
    (((y - bar[1] - 7.5) / travel).clamp(0.0, 1.0) * most as f32).round() as usize
}

/// PatternEncodingTermScreen's panel below the rows (`foot`): the mode's panel, its tabs and buttons, the cells.
pub(crate) fn draw(host: &dyn ClientHost, d: &mut dyn DrawContext, o: [f32; 2], foot: f32, mouse: [f32; 2]) {
    let b = [o[0], o[1] + foot - 71.0];
    let m = [mouse[0] - b[0], mouse[1] - b[1]];
    with(|v| {
        v.proc_scroll = v.proc_scroll.min(PROC_MOST);
        v.stone_scroll = v.stone_scroll.min(stone_most(v.options.len()));
    });
    let (mode, subs, fluids, cells, craftable, fluid_cells, result, options, pick, proc_scroll, stone_scroll) = with(|v| {
        (
            v.mode,
            v.subs,
            v.fluid_subs,
            v.cells.clone(),
            v.craftable.clone(),
            v.fluid_cells.clone(),
            v.result.clone(),
            v.options.clone(),
            v.pick,
            v.proc_scroll,
            v.stone_scroll,
        )
    });
    let panel = match mode {
        0 => [0.0, 0.0, 126.0, 68.0],
        1 => [0.0, 70.0, 126.0, 68.0],
        2 => [128.0, 70.0, 126.0, 68.0],
        _ => [0.0, 141.0, 126.0, 68.0],
    };
    crate::screens::spr(d, "ae2:gui/pattern_modes", b, [9.0, 85.0], panel);
    d.text(&crate::screens::tr(host, "gui.ae2.PatternEncoding", "Pattern Encoding"), [b[0] + 8.0, b[1] + 74.0], TEXT, false);
    for (i, (at, icon)) in TABS.iter().zip(["minecraft:crafting_table", "minecraft:furnace", "minecraft:smithing_table", "minecraft:stonecutter"]).enumerate() {
        let selected = i as i64 == mode;
        let x = b[0] + at[0] - if selected { 2.0 } else { 0.0 };
        d.gui_nine_slice(
            if selected { "widget/button_highlighted" } else { "widget/button" },
            [x, b[1] + at[1], 20.0 + if selected { 2.0 } else { 0.0 }, 20.0],
            0xFFFF_FFFF,
        );
        if let Some(st) = new_stack(host, icon, 1) {
            d.gui_draw_item_with(&st, [b[0] + at[0] + 2.0, b[1] + at[1] + 2.0], None);
        }
    }
    // The encode button (ActionItems.ENCODE: Icon.WHITE_ARROW_DOWN).
    crate::screens::state_button(d, b, ENCODE_AT, [128.0, 0.0]);
    if mode != 3 {
        half_icon(d, b, CLEAR_AT[usize::from(mode == 1)], [96.0, 0.0]);
    }
    if mode == 0 || mode == 2 {
        half_icon(d, b, SUBS_AT, if subs { [64.0, 48.0] } else { [112.0, 48.0] });
    }
    if mode == 0 {
        half_icon(d, b, FLUID_SUBS_AT, if fluids { [128.0, 48.0] } else { [144.0, 48.0] });
    }
    if mode == 1 {
        scrollbar(d, b, PROC_BAR, proc_scroll, PROC_MOST);
        if outputs_of(&cells).iter().filter(|c| c.is_some()).count() > 1 {
            half_icon(d, b, CYCLE_AT, [0.0, 240.0]);
        }
        // Icon.BACKGROUND_PRIMARY_OUTPUT in the primary output's empty slot.
        if proc_scroll == 0 && cells.get(output_cell(0)).is_none_or(Option::is_none) {
            d.sprite("ae2:gui/states", [224.0, 0.0, 16.0, 16.0], [b[0] + 110.0, b[1] + 93.0, 16.0, 16.0], Some(0x60FF_FFFF));
        }
    }
    for i in 0..places(mode) {
        let cell = place_cell(i, mode, proc_scroll);
        let Some(Some((k, n))) = cells.get(cell) else { continue };
        let at = place_at(i, mode);
        let label = (mode == 1).then(|| k.format_amount(*n, AmountFormat::Slot));
        crate::craft_screens::draw_key(host, d, k, [b[0] + at[0], b[1] + at[1]], label.as_deref());
        // The craftable mark (shouldShowCraftableIndicatorForSlot): "+" in the slot's top left.
        if craftable.get(cell).copied().unwrap_or(false) {
            crate::craft_screens::half_text(d, "+", [b[0] + at[0], b[1] + at[1]], 0xFFFF_FFFF, true);
        }
    }
    // Over the fluid substitution button: the cells it would take a fluid for, in green.
    if mode == 0 && fluids && inside(m, [FLUID_SUBS_AT[0], FLUID_SUBS_AT[1], 8.0, 8.0]) {
        for i in fluid_cells {
            let at = place_at(i, 0);
            d.fill([b[0] + at[0], b[1] + at[1], 16.0, 16.0], 0x7F00_FF00);
        }
    }
    if mode == 0 || mode == 2 {
        if let Some((k, n)) = result {
            let at = if mode == 2 { [111.0, 111.0] } else { [110.0, 111.0] };
            let label = (n > 1).then(|| n.to_string());
            crate::craft_screens::draw_key(host, d, &k, [b[0] + at[0], b[1] + at[1]], label.as_deref());
        }
    }
    if mode == 3 {
        scrollbar(d, b, STONE_BAR, stone_scroll, stone_most(options.len()));
        for (i, (k, n)) in options.iter().enumerate().skip(stone_scroll * 4).take(12) {
            let place = i - stone_scroll * 4;
            let at = [STONE_AT[0] + 16.0 * (place % 4) as f32, STONE_AT[1] + 18.0 * (place / 4) as f32];
            let src = if pick == Some(i) {
                [126.0, 159.0]
            } else if inside(m, [at[0], at[1], 16.0, 18.0]) {
                [126.0, 177.0]
            } else {
                [126.0, 141.0]
            };
            d.sprite("ae2:gui/pattern_modes", [src[0], src[1], 16.0, 18.0], [b[0] + at[0], b[1] + at[1] - 1.0, 16.0, 18.0], None);
            let label = (*n > 1).then(|| n.to_string());
            crate::craft_screens::draw_key(host, d, k, [b[0] + at[0], b[1] + at[1]], label.as_deref());
        }
    }
}

/// A press on the panel (`at` from the screen's corner): a menu button for the server, or a press the client keeps
/// (`Some(CLIENT_ONLY)`: the scroll bars, the amount dialog opening); None off the panel.
pub(crate) fn click(host: &dyn ClientHost, at: [f32; 2], foot: f32, right: bool) -> Option<i32> {
    let at = [at[0], at[1] - (foot - 71.0)];
    let client_only = crate::screens::CLIENT_ONLY;
    for (i, t) in TABS.iter().enumerate() {
        if inside(at, [t[0], t[1], 20.0, 20.0]) {
            with(|v| v.mode = i as i64);
            return Some(ENC_MODE + i as i32);
        }
    }
    let (mode, cells, options, proc_scroll, stone_scroll) = with(|v| (v.mode, v.cells.clone(), v.options.len(), v.proc_scroll, v.stone_scroll));
    if inside(at, [ENCODE_AT[0], ENCODE_AT[1], 16.0, 16.0]) {
        return Some(ENC_ENCODE);
    }
    let clear = CLEAR_AT[usize::from(mode == 1)];
    if mode != 3 && inside(at, [clear[0], clear[1], 8.0, 8.0]) {
        return Some(ENC_CLEAR);
    }
    if (mode == 0 || mode == 2) && inside(at, [SUBS_AT[0], SUBS_AT[1], 8.0, 8.0]) {
        return Some(ENC_SUBS);
    }
    if mode == 0 && inside(at, [FLUID_SUBS_AT[0], FLUID_SUBS_AT[1], 8.0, 8.0]) {
        return Some(ENC_FLUID_SUBS);
    }
    if mode == 1 {
        if outputs_of(&cells).iter().filter(|c| c.is_some()).count() > 1 && inside(at, [CYCLE_AT[0], CYCLE_AT[1], 8.0, 8.0]) {
            return Some(ENC_CYCLE);
        }
        if inside(at, [PROC_BAR[0], PROC_BAR[1], 7.0, PROC_BAR[2]]) {
            with(|v| {
                v.drag = 1;
                v.proc_scroll = bar_row(at[1], PROC_BAR, PROC_MOST);
            });
            return Some(client_only);
        }
    }
    if mode == 3 {
        if inside(at, [STONE_BAR[0], STONE_BAR[1], 7.0, STONE_BAR[2]]) {
            with(|v| {
                v.drag = 2;
                v.stone_scroll = bar_row(at[1], STONE_BAR, stone_most(options));
            });
            return Some(client_only);
        }
        for place in 0..12 {
            let i = stone_scroll * 4 + place;
            let p = [STONE_AT[0] + 16.0 * (place % 4) as f32, STONE_AT[1] + 18.0 * (place / 4) as f32];
            if i < options && inside(at, [p[0], p[1], 16.0, 18.0]) {
                with(|v| v.pick = Some(i));
                return Some(ENC_STONECUT + i as i32);
            }
        }
    }
    for i in 0..places(mode) {
        let p = place_at(i, mode);
        if inside(at, [p[0], p[1], 16.0, 16.0]) {
            let cell = place_cell(i, mode, proc_scroll);
            // The pick-block button on a set processing cell: SetProcessingPatternAmountScreen.
            let _ = host;
            return Some(ENC_GHOST | ((cell as i32) << 3) | i32::from(right));
        }
    }
    None
}

/// The middle button over a set processing cell opens the amount dialog (PatternEncodingTermScreen.mouseClicked
/// with the pick-block key). `at` from the screen's corner.
pub(crate) fn middle_click(at: [f32; 2], foot: f32) -> bool {
    let at = [at[0], at[1] - (foot - 71.0)];
    let (mode, cells, scroll) = with(|v| (v.mode, v.cells.clone(), v.proc_scroll));
    if mode != 1 {
        return false;
    }
    for i in 0..places(1) {
        let p = place_at(i, 1);
        if inside(at, [p[0], p[1], 16.0, 16.0]) {
            let cell = place_cell(i, 1, scroll);
            if let Some(Some((key, n))) = cells.get(cell) {
                let text = dialog_text(key, *n);
                with(|v| v.dialog = Some(Dialog { cell, key: key.clone(), text }));
                return true;
            }
        }
    }
    false
}

/// The amount as the dialog's field shows it: whole items, buckets for a fluid (to the mB).
fn dialog_text(key: &AeKey, amount: i64) -> String {
    let unit = key.key_type().amount_per_unit().max(1);
    if unit == 1 {
        return amount.to_string();
    }
    let text = format!("{:.3}", amount as f64 / unit as f64);
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// The dialog's field as an amount (NumberEntryWidget: an expression; a fluid's in buckets), 0..=999 999 units.
pub(crate) fn dialog_amount(key: &AeKey, text: &str) -> Option<i64> {
    let unit = key.key_type().amount_per_unit().max(1);
    let most = MAX_UNITS * unit;
    if unit == 1 {
        return crate::mathexpr::whole(text, i64::MIN, i64::MAX).map(|n| n.clamp(0, most));
    }
    let v = crate::mathexpr::value(text)?;
    v.is_finite().then(|| ((v * unit as f64).round() as i64).clamp(0, most))
}

/// The wheel over the panel: the processing inputs' or the stonecutter's rows (their scroll bars don't take the
/// wheel elsewhere). `at` from the screen's corner.
pub(crate) fn scrolled(at: [f32; 2], foot: f32, dy: f64) -> bool {
    let at = [at[0], at[1] - (foot - 71.0)];
    if !inside(at, [9.0, 85.0, 126.0, 68.0]) || dy == 0.0 {
        return false;
    }
    with(|v| {
        let step = |s: usize, most: usize| if dy > 0.0 { s.saturating_sub(1) } else { (s + 1).min(most) };
        match v.mode {
            1 => {
                v.proc_scroll = step(v.proc_scroll, PROC_MOST);
                true
            }
            3 => {
                v.stone_scroll = step(v.stone_scroll, stone_most(v.options.len()));
                true
            }
            _ => false,
        }
    })
}

/// A drag with a scroll bar held. `y` from the screen's corner.
pub(crate) fn dragged(y: f32, foot: f32) -> bool {
    let y = y - (foot - 71.0);
    with(|v| match v.drag {
        1 => {
            v.proc_scroll = bar_row(y, PROC_BAR, PROC_MOST);
            true
        }
        2 => {
            v.stone_scroll = bar_row(y, STONE_BAR, stone_most(v.options.len()));
            true
        }
        _ => false,
    })
}

pub(crate) fn released() {
    with(|v| v.drag = 0);
}

/// The panel's tooltips (`at` from the screen's corner): the tabs, the buttons, the cells (with processing's amount
/// and the dialog's hint), the outputs' roles, the stonecutter's results.
pub(crate) fn tooltip(host: &dyn ClientHost, at: [f32; 2], foot: f32, lines: &mut Vec<String>) -> bool {
    let at = [at[0], at[1] - (foot - 71.0)];
    let tr = |k: &str, f: &str| crate::screens::tr(host, k, f);
    let (mode, subs, fluids, cells, craftable, options, proc_scroll, stone_scroll) =
        with(|v| (v.mode, v.subs, v.fluid_subs, v.cells.clone(), v.craftable.clone(), v.options.clone(), v.proc_scroll, v.stone_scroll));
    let tabs = [
        ("gui.ae2.CraftingPattern", "Crafting Patterns"),
        ("gui.ae2.ProcessingPattern", "Processing Patterns"),
        ("gui.ae2.SmithingTablePattern", "Smithing Table Patterns"),
        ("gui.ae2.StonecuttingPattern", "Stonecutting Patterns"),
    ];
    for (t, (k, f)) in TABS.iter().zip(tabs) {
        if inside(at, [t[0], t[1], 20.0, 20.0]) {
            lines.push(tr(k, f));
            return true;
        }
    }
    let mut add = |name: String, desc: String| {
        lines.push(name);
        lines.extend(desc.split('\n').map(|l| format!("§7{l}")));
    };
    if inside(at, [ENCODE_AT[0], ENCODE_AT[1], 16.0, 16.0]) {
        add(
            tr("gui.tooltips.ae2.Encode", "Encode Pattern"),
            tr("gui.tooltips.ae2.EncodeDescription", "Write the entered pattern to the current encoded pattern, or to available blank pattern."),
        );
        return true;
    }
    let clear = CLEAR_AT[usize::from(mode == 1)];
    if mode != 3 && inside(at, [clear[0], clear[1], 8.0, 8.0]) {
        add(tr("gui.tooltips.ae2.Clear", "Clear"), tr("gui.tooltips.ae2.ClearSettings", "Clear Config/Settings"));
        return true;
    }
    if (mode == 0 || mode == 2) && inside(at, [SUBS_AT[0], SUBS_AT[1], 8.0, 8.0]) {
        if subs {
            add(
                tr("gui.tooltips.ae2.SubstitutionsOn", "Substitutions Enabled"),
                tr("gui.tooltips.ae2.SubstitutionsDescEnabled", "Click to prevent alternate items in the recipe."),
            );
        } else {
            add(
                tr("gui.tooltips.ae2.SubstitutionsOff", "Substitutions Disabled"),
                tr("gui.tooltips.ae2.SubstitutionsDescDisabled", "Click to allow alternate items in the recipe."),
            );
        }
        return true;
    }
    if mode == 0 && inside(at, [FLUID_SUBS_AT[0], FLUID_SUBS_AT[1], 8.0, 8.0]) {
        let desc = if fluids {
            tr("gui.tooltips.ae2.FluidSubstitutionsDescEnabled", "Try to use fluids instead of items.\nSubstitutable ingredients are marked in green.")
        } else {
            tr("gui.tooltips.ae2.FluidSubstitutionsDescDisabled", "Don't use fluids.")
        };
        add(tr("gui.tooltips.ae2.FluidSubstitutions", "Fluid Substitutions"), desc);
        return true;
    }
    if mode == 1 && outputs_of(&cells).iter().filter(|c| c.is_some()).count() > 1 && inside(at, [CYCLE_AT[0], CYCLE_AT[1], 8.0, 8.0]) {
        add(
            tr("gui.tooltips.ae2.CycleProcessingOutput", "Cycle Outputs"),
            tr("gui.tooltips.ae2.CycleProcessingOutputTooltip", "Change the primary output of this pattern"),
        );
        return true;
    }
    for i in 0..places(mode) {
        let p = place_at(i, mode);
        if !inside(at, [p[0], p[1], 16.0, 16.0]) {
            continue;
        }
        let cell = place_cell(i, mode, proc_scroll);
        match cells.get(cell).cloned().flatten() {
            Some((k, n)) => {
                lines.push(crate::screens::key_name(host, &k));
                if craftable.get(cell).copied().unwrap_or(false) {
                    lines.push(format!("§8{}", tr("gui.tooltips.ae2.Craftable", "Craftable")));
                }
                if mode == 1 {
                    lines.push(format!("§7Amount: {}", k.format_amount(n, AmountFormat::Full)));
                    lines.push(format!("§7{}: Set amount", tr("gui.tooltips.ae2.MiddleClick", "Middle-Click")));
                }
            }
            None if mode == 1 && i >= 9 => {
                // processing-primary-output / processing-optional-output*.
                let primary = cell == output_cell(0);
                let (t, h) = if primary {
                    ("primary_processing_result_tooltip", "primary_processing_result_hint")
                } else {
                    ("secondary_processing_result_tooltip", "secondary_processing_result_hint")
                };
                let fall = if primary {
                    ("Primary Processing Result", "Can be requested through the automated crafting system.")
                } else {
                    (
                        "Secondary Processing Result",
                        "Can not be directly requested through the automated crafting system, but will be used before stored items in multi-step recipes.",
                    )
                };
                lines.push(tr(&format!("gui.ae2.PatternEncoding.{t}"), fall.0));
                lines.push(format!("§8{}", tr(&format!("gui.ae2.PatternEncoding.{h}"), fall.1)));
            }
            None => {}
        }
        return true;
    }
    if mode == 3 {
        for place in 0..12 {
            let i = stone_scroll * 4 + place;
            let p = [STONE_AT[0] + 16.0 * (place % 4) as f32, STONE_AT[1] + 18.0 * (place / 4) as f32];
            if let Some((k, _)) = options.get(i)
                && inside(at, [p[0], p[1], 16.0, 18.0])
            {
                lines.push(crate::screens::key_name(host, k));
                return true;
            }
        }
    }
    inside(at, [9.0, 85.0, 126.0, 68.0])
}

/// The pointer as the screen hooks last saw it (the block screen's draw isn't told it).
static MOUSE: Mutex<[f32; 2]> = Mutex::new([-1.0, -1.0]);

pub(crate) fn note_mouse(at: [f32; 2]) {
    *MOUSE.lock().unwrap_or_else(|e| e.into_inner()) = at;
}

pub(crate) fn mouse() -> [f32; 2] {
    *MOUSE.lock().unwrap_or_else(|e| e.into_inner())
}

// ---- Client: the amount dialog (SetProcessingPatternAmountScreen over the terminal)

pub(crate) fn dialog_open() -> bool {
    with(|v| v.dialog.is_some())
}

fn dialog_origin(rect: [f32; 4]) -> [f32; 2] {
    [rect[0] + 9.0, rect[1] + 17.0]
}

pub(crate) fn dialog_draw(host: &dyn ClientHost, d: &mut dyn DrawContext, rect: [f32; 4], icon: &str) {
    let Some(dialog) = with(|v| v.dialog.clone()) else { return };
    let size = d.size();
    d.fill([0.0, 0.0, size[0], size[1]], 0x8010_1010);
    let o = dialog_origin(rect);
    crate::screens::spr(d, "ae2:gui/craft_amt", o, [0.0, 0.0], [0.0, 0.0, 176.0, 107.0]);
    d.text(&crate::screens::tr(host, "gui.ae2.SelectAmount", "Select Amount"), [o[0] + 8.0, o[1] + 6.0], TEXT, false);
    for (x, w, n) in crate::craft_screens::STEPS {
        crate::craft_screens::button(d, [o[0] + x, o[1] + 30.0, w, 20.0], &format!("+{n}"), true);
        crate::craft_screens::button(d, [o[0] + x, o[1] + 72.0, w, 20.0], &format!("-{n}"), true);
    }
    d.fill([o[0] + 47.0, o[1] + 54.0, 68.0, 14.0], 0xFFFF_FFFF);
    d.fill([o[0] + 48.0, o[1] + 55.0, 66.0, 12.0], 0xFF00_0000);
    let color = if dialog_amount(&dialog.key, &dialog.text).is_some() { 0xFFFF_FFFF } else { 0xFFFF_6060 };
    d.text(&format!("{}_", dialog.text), [o[0] + 50.0, o[1] + 57.0], color, false);
    crate::craft_screens::draw_key(host, d, &dialog.key, [o[0] + 23.0, o[1] + 53.0], None);
    let s = crate::craft_screens::NEXT_BUTTON;
    crate::craft_screens::button(d, [o[0] + s[0], o[1] + s[1], s[2], s[3]], &crate::screens::tr(host, "gui.ae2.Set", "Set"), true);
    // The back tab (TabButton with the terminal's icon).
    d.sprite("ae2:gui/states", [128.0, 192.0, 25.0, 22.0], [o[0] + BACK_TAB[0], o[1] + BACK_TAB[1], 25.0, 22.0], None);
    if let Some(st) = new_stack(host, icon, 1) {
        d.gui_draw_item_with(&st, [o[0] + BACK_TAB[0] + 4.0, o[1] + BACK_TAB[1] + 3.0], None);
    }
}

fn dialog_confirm() {
    let Some(dialog) = with(|v| v.dialog.clone()) else { return };
    let Some(n) = dialog_amount(&dialog.key, &dialog.text) else { return };
    // Shown at once; the server sets it (0 empties the cell).
    with(|v| {
        if let Some(c) = v.cells.get_mut(dialog.cell) {
            *c = (n > 0).then(|| (dialog.key.clone(), n));
        }
        v.dialog = None;
    });
    crate::craft_screens::send(format!("encamt {} {n}", dialog.cell));
}

/// A press while the dialog is open: it takes them all. `mouse` and `rect` in GUI pixels.
pub(crate) fn dialog_pressed(mouse: [f32; 2], rect: [f32; 4]) {
    let Some(dialog) = with(|v| v.dialog.clone()) else { return };
    let o = dialog_origin(rect);
    let at = [mouse[0] - o[0], mouse[1] - o[1]];
    if inside(at, BACK_TAB) || !inside(at, [0.0, 0.0, 176.0, 107.0]) {
        with(|v| v.dialog = None);
        return;
    }
    let s = crate::craft_screens::NEXT_BUTTON;
    if inside(at, s) {
        dialog_confirm();
        return;
    }
    let unit = dialog.key.key_type().amount_per_unit().max(1);
    let now = dialog_amount(&dialog.key, &dialog.text).unwrap_or(0);
    for (x, w, step) in crate::craft_screens::STEPS {
        let change = if inside(at, [x, 30.0, w, 20.0]) {
            step * unit
        } else if inside(at, [x, 72.0, w, 20.0]) {
            -step * unit
        } else {
            continue;
        };
        let n = (now + change).clamp(0, MAX_UNITS * unit);
        with(|v| {
            if let Some(d) = v.dialog.as_mut() {
                d.text = dialog_text(&dialog.key, n);
            }
        });
    }
}

/// Keys while the dialog is open: backspace, Enter (Set), Escape (back).
pub(crate) fn dialog_key(key: &str) {
    match key {
        "key.keyboard.escape" => with(|v| v.dialog = None),
        "key.keyboard.backspace" => with(|v| {
            if let Some(d) = v.dialog.as_mut() {
                d.text.pop();
            }
        }),
        "key.keyboard.enter" | "key.keyboard.keypad.enter" => dialog_confirm(),
        _ => {}
    }
}

pub(crate) fn dialog_typed(ch: char) {
    with(|v| {
        if let Some(d) = v.dialog.as_mut()
            && crate::mathexpr::allowed(ch)
            && d.text.len() < 32
        {
            if d.text == "0" {
                d.text.clear();
            }
            d.text.push(ch);
        }
    });
}

pub(crate) fn closing() {
    with(|v| {
        v.dialog = None;
        v.drag = 0;
    });
}

/// The server's `ae2:enc` (see `view`).
pub(crate) fn from_server(text: &str) {
    let mut v = View { cells: vec![None; ENC_CELLS], craftable: vec![false; ENC_CELLS], ..View::default() };
    for l in text.lines() {
        let mut w = l.split(' ');
        match w.next() {
            Some("m") => {
                let mut n = || w.next().and_then(|x| x.parse::<i64>().ok()).unwrap_or(0);
                v.mode = n().clamp(0, 3);
                v.subs = n() != 0;
                v.fluid_subs = n() != 0;
                v.pick = usize::try_from(n()).ok();
            }
            Some("c") => {
                let (Some(i), Some(n), Some(k)) =
                    (w.next().and_then(|x| x.parse::<usize>().ok()), w.next().and_then(|x| x.parse::<i64>().ok()), w.next().and_then(key_from_string))
                else {
                    continue;
                };
                if i < ENC_CELLS {
                    v.cells[i] = Some((k, n));
                    v.craftable[i] = w.next() == Some("+");
                }
            }
            Some("r") => v.result = w.next().and_then(|n| n.parse().ok()).zip(w.next().and_then(key_from_string)).map(|(n, k)| (k, n)),
            Some("o") => {
                if let (Some(n), Some(k)) = (w.next().and_then(|n| n.parse().ok()), w.next().and_then(key_from_string)) {
                    v.options.push((k, n));
                }
            }
            Some("f") => v.fluid_cells.extend(w.next().and_then(|i| i.parse::<usize>().ok()).filter(|i| *i < 9)),
            _ => {}
        }
    }
    with(|old| {
        // The client's own state stays: the scroll bars, a held bar, the open dialog.
        v.proc_scroll = old.proc_scroll;
        v.stone_scroll = old.stone_scroll;
        v.drag = old.drag;
        v.dialog = old.dialog.take();
        *old = v;
    });
}

// ---- Client: the encoded pattern's look

/// AE2's encoded pattern with shift held (EncodedPatternBakedModel): the item it makes over it.
pub(crate) fn pattern_decoration(host: &dyn ClientHost, event: &mut ItemDecorationsEvent<'_>) {
    if !(host.is_key_down("key.keyboard.left.shift") || host.is_key_down("key.keyboard.right.shift")) {
        return;
    }
    let Some((_, outputs, _)) = crate::autocraft::pattern_lists(event.stack) else { return };
    let Some((out, _)) = outputs.first() else { return };
    let at = [event.x, event.y];
    event.draw.fill([at[0], at[1], 16.0, 16.0], 0xFF8B_8B8B);
    if out.key_type() == KeyType::Fluids {
        event.draw.fluid(out.id(), at, 16.0);
    } else if let Some(st) = stack_of(host, out, 1) {
        event.draw.gui_draw_item_with(&st, at, None);
    }
}

/// Patterns' item part: the look with shift held.
pub(crate) struct PatternLook;

#[mod_api::sunrise_part]
impl ItemPart for PatternLook {
    fn item_decorations(&self, host: &dyn ClientHost, event: &mut ItemDecorationsEvent<'_>) {
        pattern_decoration(host, event);
    }
}

pub(crate) fn register_parts(r: &mut dyn Registrar) {
    for id in ["ae2:crafting_pattern", "ae2:processing_pattern", "ae2:smithing_table_pattern", "ae2:stonecutting_pattern"] {
        let k = r.item_key(id);
        r.item_part(k, PatternLook);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str) -> AeKey {
        AeKey::item(id, "", 0, 0)
    }

    #[test]
    fn cells_keep_the_old_twelve() {
        // The twelve-cell encoder saved nine inputs then three outputs: they stay where they were.
        assert_eq!((0..9).map(input_cell).collect::<Vec<_>>(), (0..9).collect::<Vec<_>>());
        assert_eq!((0..3).map(output_cell).collect::<Vec<_>>(), vec![9, 10, 11]);
        assert_eq!(input_cell(9), 12);
        assert_eq!(input_cell(80), 83);
        assert_eq!(output_cell(3), 84);
        assert_eq!(output_cell(26), 107);
        // Every cell is used once.
        let mut all: Vec<usize> = (0..INPUTS).map(input_cell).chain((0..OUTPUTS).map(output_cell)).collect();
        all.sort_unstable();
        assert_eq!(all, (0..ENC_CELLS).collect::<Vec<_>>());
    }

    #[test]
    fn processing_places_scroll() {
        assert_eq!(place_cell(0, 1, 0), 0);
        assert_eq!(place_cell(8, 1, 0), 8);
        assert_eq!(place_cell(0, 1, 1), input_cell(3));
        assert_eq!(place_cell(8, 1, PROC_MOST), input_cell(80));
        assert_eq!(place_cell(9, 1, 0), output_cell(0));
        assert_eq!(place_cell(11, 1, 24), output_cell(26));
        // Crafting's cells are the first nine.
        assert_eq!(place_cell(4, 0, 7), 4);
    }

    #[test]
    fn cycle_moves_the_primary_output() {
        let a = vec![Some(1), None, Some(2), Some(3), None];
        assert_eq!(cycle(&a), vec![Some(2), None, Some(3), Some(1), None]);
        // One set output has nothing to cycle with (canCycleProcessingOutputs: the button needs two).
        assert_eq!(cycle(&[Some(1), Some(4)]), vec![Some(4), Some(1)]);
        assert_eq!(cycle::<i32>(&[None, None]), vec![None, None]);
    }

    #[test]
    fn condensed_inputs() {
        let list = vec![(item("a:x"), 2), (item("a:y"), 1), (item("a:x"), 3)];
        assert_eq!(condense(list), vec![(item("a:x"), 5), (item("a:y"), 1)]);
    }

    #[test]
    fn crafting_takes_one_item_each() {
        let mut cells: Vec<Cell> = vec![None; ENC_CELLS];
        cells[0] = Some((item("a:x"), 12));
        cells[4] = Some((AeKey::fluid("minecraft:water", ""), 1000));
        cells[20] = Some((item("a:y"), 5));
        cells[9] = Some((item("a:out"), 7));
        fix_crafting(&mut cells);
        assert_eq!(cells[0], Some((item("a:x"), 1)));
        assert_eq!(cells[4], None);
        assert_eq!(cells[20], Some((item("a:y"), 1)));
        // Outputs aren't inputs.
        assert_eq!(cells[9], Some((item("a:out"), 7)));
    }

    #[test]
    fn amounts_are_clamped() {
        assert_eq!(amount_for(&item("a:x"), 0), None);
        assert_eq!(amount_for(&item("a:x"), -5), None);
        assert_eq!(amount_for(&item("a:x"), 64), Some(64));
        assert_eq!(amount_for(&item("a:x"), 5_000_000), Some(999_999));
        let water = AeKey::fluid("minecraft:water", "");
        assert_eq!(amount_for(&water, 5_000_000_000), Some(999_999_000));
        assert_eq!(dialog_amount(&item("a:x"), "2*64"), Some(128));
        assert_eq!(dialog_amount(&water, "1.5"), Some(1500));
        assert_eq!(dialog_amount(&water, "0.0005"), Some(1));
        assert_eq!(dialog_text(&water, 1500), "1.5");
        assert_eq!(dialog_text(&water, 2000), "2");
        assert_eq!(dialog_amount(&item("a:x"), "-3"), Some(0));
    }

    #[test]
    fn processing_needs_the_primary_output() {
        let x = Some((item("a:x"), 1));
        assert!(processing_ready(&[x.clone()], &[x.clone(), None]));
        assert!(!processing_ready(&[x.clone()], &[None, x.clone()]));
        assert!(!processing_ready(&[None], &[x.clone()]));
    }

    #[test]
    fn mode_cells() {
        assert!(mode_takes(0, 8) && !mode_takes(0, 9));
        assert!(mode_takes(1, 107) && !mode_takes(1, 108));
        assert!(mode_takes(2, 2) && !mode_takes(2, 3));
        assert!(mode_takes(3, 0) && !mode_takes(3, 1));
    }

    #[test]
    fn ghost_clicks() {
        let x = item("a:x");
        let water = AeKey::fluid("minecraft:water", "");
        let held = |n| Some((x.clone(), n, None));
        // Crafting: one of it, whatever the stack.
        assert_eq!(ghost_change(None, held(32), false, false), Some((x.clone(), 1)));
        // Processing: the stack's count; right-click adds one of the same, else one.
        assert_eq!(ghost_change(None, held(32), false, true), Some((x.clone(), 32)));
        assert_eq!(ghost_change(Some((x.clone(), 3)), held(32), true, true), Some((x.clone(), 4)));
        assert_eq!(ghost_change(Some((item("a:y"), 3)), held(32), true, true), Some((x.clone(), 1)));
        // A bucket right-clicked in processing: its fluid, a bucket's worth; left: the bucket itself.
        let bucket = Some((item("minecraft:water_bucket"), 1, Some(water.clone())));
        assert_eq!(ghost_change(None, bucket.clone(), true, true), Some((water, 1000)));
        assert_eq!(ghost_change(None, bucket.clone(), false, true), Some((item("minecraft:water_bucket"), 1)));
        // Empty cursor: clear; right in processing takes one away.
        assert_eq!(ghost_change(Some((x.clone(), 3)), None, false, true), None);
        assert_eq!(ghost_change(Some((x.clone(), 3)), None, true, true), Some((x.clone(), 2)));
        assert_eq!(ghost_change(Some((x.clone(), 1)), None, true, true), None);
    }

    #[test]
    fn stonecut_choices_in_order() {
        let mut list = vec![
            ("minecraft:stone_stairs_from_stone".to_owned(), item("minecraft:stone_stairs"), 1),
            ("minecraft:stone_bricks_from_stone".to_owned(), item("minecraft:stone_bricks"), 1),
            ("ae2:x".to_owned(), item("ae2:quartz_block"), 1),
            ("minecraft:stone_slab_from_stone".to_owned(), item("minecraft:stone_slab"), 2),
        ];
        stonecut_sort(&mut list);
        let ids: Vec<&str> = list.iter().map(|(_, k, _)| k.id()).collect();
        assert_eq!(ids, vec!["ae2:quartz_block", "minecraft:stone_bricks", "minecraft:stone_slab", "minecraft:stone_stairs"]);
        // The pick is the recipe's id, found again in the list (or forgotten).
        let mut nbt = Compound::new();
        set_nbt_string(&mut nbt, "stonecut", Some("minecraft:stone_slab_from_stone"));
        assert_eq!(stonecut_pick(&nbt, &list), Some(2));
        set_nbt_string(&mut nbt, "stonecut", Some("minecraft:gone"));
        assert_eq!(stonecut_pick(&nbt, &list), None);
        assert_eq!(stone_most(12), 0);
        assert_eq!(stone_most(13), 1);
    }

    #[test]
    fn sparse_round_trip() {
        let list = vec![(0, item("a:x"), 3), (40, AeKey::fluid("minecraft:lava", ""), 1500)];
        let mut p = Compound::new();
        p.insert(java("sparse_in"), sparse_entries(&list));
        assert_eq!(read_sparse(&p, "sparse_in"), list);
    }
}
