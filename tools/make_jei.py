"""Writes AE2's JEI categories as jei-fork category files (assets/ae2/jei/categories/*.json), at the places AE2's
JEI plugin (appeng.integration.modules.jei) puts its slots. Inscriber and charger read their recipe types; transform,
entropy, certus growth and condenser are written out from data/ae2/recipe with each card's note (JEI's files can't
pick a text by a recipe's value). Run from anywhere: python tools/make_jei.py"""
import json, pathlib

ROOT = pathlib.Path(__file__).resolve().parent.parent
OUT = ROOT / "assets/ae2/jei/categories"
RECIPES = ROOT / "data/ae2/recipe"
K = "ae2.rei_jei_integration."


def write(name, doc):
    OUT.mkdir(parents=True, exist_ok=True)
    (OUT / f"{name}.json").write_text(json.dumps(doc, indent=2) + "\n", encoding="utf-8")


def load(folder):
    return [json.loads(p.read_text(encoding="utf-8")) for p in sorted((RECIPES / folder).rglob("*.json"))]


# InscriberRecipeCategory: inscriber.png from (44, 15), 97x64: top (1, 1), middle (19, 24), bottom (1, 47), out (69, 25).
write("inscriber", {
    "id": "ae2:inscriber", "title": "block.ae2.inscriber", "icon": "ae2:inscriber", "catalysts": ["ae2:inscriber"],
    "recipe_type": "ae2:inscriber", "size": [97, 64], "arrow": [44, 24],
    "slots": [
        {"x": 1, "y": 1, "role": "input", "path": "ingredients.top"},
        {"x": 19, "y": 24, "role": "input", "path": "ingredients.middle"},
        {"x": 1, "y": 47, "role": "input", "path": "ingredients.bottom"},
        {"x": 69, "y": 25, "role": "output", "path": "result"},
    ],
})

# ChargerCategory: 130x50, the item at (31, 8), the result at (81, 8), the power line under them.
write("charger", {
    "id": "ae2:charger", "title": "block.ae2.charger", "icon": "ae2:charger", "catalysts": ["ae2:charger", "ae2:crank"],
    "recipe_type": "ae2:charger", "size": [130, 50], "arrow": [52, 8],
    "slots": [
        {"x": 31, "y": 8, "role": "input", "path": "ingredient"},
        {"x": 81, "y": 8, "role": "output", "path": "result"},
    ],
    "texts": [{"x": 3, "y": 36, "text": "10 turns or 1600 AE"}],
})

# TransformCategory: the ingredients in a column, the circumstance (water or an explosion) in the middle, the result.
transform = []
for r in load("transform"):
    c = r.get("circumstance", {})
    explosion = c.get("type") == "explosion"
    transform.append({
        "ingredients": r["ingredients"], "result": r["result"],
        "via": "minecraft:tnt" if explosion else "minecraft:water_bucket",
        "note": K + ("explosion" if explosion else "submerge_in"),
        "water": "" if explosion else "block.minecraft.water",
    })
write("transform", {
    "id": "ae2:item_transformation", "title": K + "transform_category", "icon": "minecraft:water_bucket",
    "catalysts": ["minecraft:water_bucket"], "recipes": transform, "size": [130, 64], "arrow": [30, 23],
    "slots": [
        {"x": 6, "y": 4, "role": "input", "path": "ingredients", "columns": 1, "step": 18, "max": 3},
        {"x": 56, "y": 23, "role": "other", "path": "via"},
        {"x": 104, "y": 23, "role": "output", "path": "result"},
    ],
    "texts": [{"x": 34, "y": 46, "text": "%s %s", "args": ["note", "water"], "tr_args": [0, 1]}],
})

# EntropyManipulatorCategory: the block or fluid, heat or cool, what it becomes and drops.
entropy = []
for r in load("entropy"):
    i = r["input"].get("block") or r["input"].get("fluid")
    out = r.get("output", {})
    results = []
    b = out.get("block", out.get("fluid", {})).get("id")
    if b and b != "minecraft:air":
        results.append({"id": {"minecraft:water": "minecraft:water_bucket", "minecraft:flowing_water": "minecraft:water_bucket",
                               "minecraft:lava": "minecraft:lava_bucket"}.get(b, b)})
    results += [{"id": d["item"]} for d in out.get("drops", [])]
    src = {"minecraft:water": "minecraft:water_bucket", "minecraft:flowing_water": "minecraft:water_bucket",
           "minecraft:lava": "minecraft:lava_bucket"}.get(i["id"], i["id"])
    cool = r.get("mode") == "cool"
    entropy.append({"input": src, "results": results,
                    "mode": ("Cool" if cool else "Heat") + " (1600 AE)",
                    "click": K + ("shift_right_click" if cool else "right_click")})
write("entropy", {
    "id": "ae2:entropy", "title": "item.ae2.entropy_manipulator", "icon": "ae2:entropy_manipulator",
    "catalysts": ["ae2:entropy_manipulator"], "recipes": entropy, "size": [130, 50], "arrow": [52, 10],
    "slots": [
        {"x": 31, "y": 10, "role": "input", "path": "input"},
        {"x": 81, "y": 10, "role": "output", "path": "results", "columns": 2, "step": 18},
    ],
    "texts": [{"x": 3, "y": 32, "text": "%s", "args": ["mode"]},
              {"x": 3, "y": 42, "text": "%s", "args": ["click"], "tr_args": [0]}],
})

# CertusGrowthCategory's pages.
def step(a, b, *notes, count=1):
    d = {"input": a, "result": {"id": b, "count": count} if b else None}
    for n, key in enumerate(notes):
        d[f"n{n}"] = K + key
    return d
growth = [step("ae2:flawless_budding_quartz", "ae2:small_quartz_bud", "quartz_buds_grow_on_budding_quartz", "flawless_budding_quartz_description")]
growth += [step(f"ae2:{b}_budding_quartz", "ae2:small_quartz_bud", "quartz_buds_grow_on_budding_quartz") for b in ("flawed", "chipped", "damaged")]
growth += [step(a, b, "crystal_growth_accelerators_effect") for a, b in
           (("ae2:small_quartz_bud", "ae2:medium_quartz_bud"), ("ae2:medium_quartz_bud", "ae2:large_quartz_bud"), ("ae2:large_quartz_bud", "ae2:quartz_cluster"))]
growth.append(step("ae2:quartz_cluster", "ae2:certus_quartz_crystal", "fully_grown_buds_drop_crystals", "fortune_applies", count=4))
growth += [step(a, b, "imperfect_budding_quartz_decays") for a, b in
           (("ae2:flawed_budding_quartz", "ae2:chipped_budding_quartz"), ("ae2:chipped_budding_quartz", "ae2:damaged_budding_quartz"), ("ae2:damaged_budding_quartz", "ae2:quartz_block"))]
growth.append(step(["ae2:small_quartz_bud", "ae2:medium_quartz_bud", "ae2:large_quartz_bud"], "ae2:certus_quartz_dust", "buds_drop_dust_when_not_fully_grown"))
growth.append(step("ae2:flawless_budding_quartz", "ae2:flawed_budding_quartz", "budding_quartz_decays_when_broken", "silk_touch_prevents_decay_for_imperfect", "spatial_io_never_causes_any_decay"))
growth.append(step("ae2:charged_certus_quartz_crystal", "ae2:damaged_budding_quartz", "budding_quartz_creation_and_worldgen"))
for g in growth:
    if g["result"] is None:
        del g["result"]
write("certus_growth", {
    "id": "ae2:certus_growth", "title": K + "certus_quartz_growth", "icon": "ae2:flawless_budding_quartz",
    "catalysts": ["ae2:growth_accelerator"], "recipes": growth, "size": [150, 72], "arrow": [52, 6],
    "slots": [
        {"x": 31, "y": 6, "role": "input", "path": "input"},
        {"x": 81, "y": 6, "role": "output", "path": "result"},
    ],
    "texts": [{"x": 2, "y": 30 + 12 * n, "text": "%s", "args": [f"n{n}"], "tr_args": [0], "needs": f"n{n}"} for n in range(3)],
})

# CondenserCategory: condenser.png from (50, 25), 94x48: the component, the result.
write("condenser", {
    "id": "ae2:condenser", "title": "block.ae2.condenser", "icon": "ae2:condenser", "catalysts": ["ae2:condenser"],
    "recipes": [
        {"component": "ae2:cell_component_1k", "result": {"id": "ae2:matter_ball"}, "note": "256 per item"},
        {"component": "ae2:cell_component_64k", "result": {"id": "ae2:singularity"}, "note": "256,000 per item"},
    ],
    "size": [94, 48], "arrow": [28, 6],
    "slots": [
        {"x": 6, "y": 6, "role": "other", "path": "component"},
        {"x": 64, "y": 6, "role": "output", "path": "result"},
    ],
    "texts": [{"x": 2, "y": 32, "text": "%s", "args": ["note"]}],
})
print("written", sorted(p.name for p in OUT.iterdir()))
