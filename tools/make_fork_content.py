"""Writes AE2's assets/ and data/ for the fork build (ae2-fork/), from the upstream content mod.

The upstream content (`../ae2-content`, written by `../ae2-horizon/tools/make_content_mod.py` from the Fabric jar;
the facades need a Horizon cache, so it is read, not regenerated here) is reused as it is, with what the fork reads
differently:

- no `horizon-mod.toml` inside: the fork mod's own manifest is the only one;
- the W guide's pages move from `data/ae2/ae2guide/` to `assets/ae2/ae2guide/` (GuideME reads assets on the client);
- shape tables name their properties as a list (`"properties": ["facing"]`, the fork refuses `"property"`);
- the storage cells' status light reads the item tint source `ae2:cell_status` (the fork's `item_tint_source`, from
  the cell's own data) instead of `minecraft:custom_model_data` written on the stack;
- the content's `data/ae2/horizon/` (block, item, menu declarations) is Sunrise's `data/ae2/sunrise/`;
- the cables' models and blockstates are AE2's own cable baking (`make_cables.py`), and each cable block has `x_core`;
- `src/blocks_gen.rs`: every block whose template brings a vanilla block entity (beacon, barrel, shulker box, skull),
  so the mod declares its own block entity type for each and the vanilla one never appears (no beacon beam, no
  skull renderer).

Usage: python make_fork_content.py [content folder]   (writes ../assets, ../data and ../src/blocks_gen.rs)
"""

import json
import os
import shutil
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import make_cables  # noqa: E402

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
SOURCE = os.path.join(ROOT, "..", "ae2-content")
ID = "ae2"
# Templates whose vanilla block carries a block entity of its own.
BE_TEMPLATES = ("minecraft:beacon", "minecraft:barrel", "minecraft:shulker_box", "minecraft:skeleton_skull")


def fix_shapes(table):
    if isinstance(table, dict) and "property" in table:
        table = dict(table)
        table["properties"] = [table.pop("property")]
    return table


def CELL_ITEM(name):
    return name.startswith(("item_storage_cell_", "fluid_storage_cell_"))


def cell_tint(data):
    """The cell's light (tint 1) from the `ae2:cell_status` tint source."""
    model = json.loads(data)
    for tint in model.get("model", {}).get("tints", []):
        if tint.get("type") == "minecraft:custom_model_data":
            tint.clear()
            tint.update({"type": "ae2:cell_status", "default": 65280})
    return (json.dumps(model, indent=2) + "\n").encode()


# Guide pictures of things a page already shows as a 3D scene: dropped (the scene stays). AE2's diagrams (pattern
# screens, flow charts, the P2P portal drawing, the key icons) are its authors' art and stay.
PICTURE_SWAP = {
    "![A formed Quantum Network Bridge](../assets/diagrams/quantum_bridge_demonstration.png)\n": "",
    "![Meteorite Crater](../assets/assemblies/meteorite-crater.png)": "",
}


def sunrise_path(rel):
    """The content's (and the overrides') `data/<ns>/horizon/` declarations are Sunrise's `data/<ns>/sunrise/`."""
    parts = rel.split("/")
    if len(parts) > 2 and parts[0] == "data" and parts[2] == "horizon":
        parts[2] = "sunrise"
    return "/".join(parts)


# fixG: the parts whose own block's model lies on its back (bus.rs `model_near` > 0: displays, level emitters, the
# cable interface, pattern provider and energy acceptor, the cable anchor). Placed against a block they now face it,
# as AE2's PartPlacement puts a part on the side toward the clicked block (`facing` names that side, as a cell's part
# side does: a pattern provider pushes into it, the back joins a cable): `x_front` true. Their model then sits against
# that block, screen to the player: the back-lying model turned as for the opposite facing. `x_front` false (every
# block placed before, the cable bus's own draw, a part placed on a cable) keeps the old meaning: `facing` the screen's
# way, the back on the block behind it.
FRONT_MOUNTED = (
    "terminal", "crafting_terminal", "pattern_encoding_terminal", "pattern_access_terminal", "storage_monitor",
    "conversion_monitor", "monitor", "semi_dark_monitor", "dark_monitor", "hz_monitor_lit", "hz_semi_dark_monitor_lit",
    "hz_dark_monitor_lit", "level_emitter", "energy_level_emitter", "cable_interface", "cable_pattern_provider",
    "cable_energy_acceptor", "cable_anchor",
)
OPPOSITE = {"down": "up", "up": "down", "north": "south", "south": "north", "west": "east", "east": "west"}


def front_mounted_blockstate(data):
    """fixG: every case gets `x_front` false; each facing gets an `x_front` true copy of its opposite's models."""
    bs = json.loads(data)
    if "multipart" in bs:
        old = bs["multipart"]
        new = []
        for front in (False, True):
            for case in old:
                when = case.get("when", {})
                if "facing" not in when:
                    if not front:
                        new.append(case)
                    continue
                c = json.loads(json.dumps(case))
                if front:
                    c["when"]["facing"] = OPPOSITE[when["facing"]]
                c["when"]["x_front"] = "true" if front else "false"
                new.append(c)
        bs["multipart"] = new
    else:
        variants = {}
        for front in (False, True):
            for key, v in bs["variants"].items():
                props = dict(p.split("=", 1) for p in key.split(",") if p)
                if front:
                    props["facing"] = OPPOSITE[props["facing"]]
                props["x_front"] = "true" if front else "false"
                variants[",".join(f"{k}={props[k]}" for k in sorted(props))] = v
        bs["variants"] = variants
    return dump_json(bs)


def front_mounted_decl(data):
    """fixG: `x_front` declared (false by default), and the shapes by facing and `x_front` (true: the opposite's)."""
    decl = json.loads(data)
    decl.setdefault("state_properties", {})["x_front"] = {"type": "boolean"}
    decl.setdefault("default_state", {})["x_front"] = "false"
    for key in ("shapes", "collision_shapes", "interaction_shapes"):
        table = decl.get(key)
        if not isinstance(table, dict) or table.get("properties") != ["facing"]:
            continue
        values = {}
        for f, boxes in table["values"].items():
            values[f"{f},false"] = boxes
            values[f"{OPPOSITE[f]},true"] = boxes
        table["properties"] = ["facing", "x_front"]
        table["values"] = values
    return json.dumps(decl, indent=2).encode()


def dump_json(obj):
    return (json.dumps(obj, indent=2) + "\n").encode()


def main(source):
    files = {}
    for tree in ("assets", "data"):
        base = os.path.join(source, tree)
        for dirpath, _, names in os.walk(base):
            for n in names:
                full = os.path.join(dirpath, n)
                rel = sunrise_path(os.path.relpath(full, source).replace(os.sep, "/"))
                with open(full, "rb") as f:
                    files[rel] = f.read()
    out = {}
    be_blocks = []
    for rel, data in files.items():
        if rel.startswith(f"data/{ID}/ae2guide/"):
            rel = rel.replace(f"data/{ID}/ae2guide/", f"assets/{ID}/ae2guide/", 1)
            if rel.endswith(".md"):
                for pic, swap in PICTURE_SWAP.items():
                    data = data.replace(pic.encode(), swap.encode())
        if rel.startswith(f"data/{ID}/sunrise/block/") and rel.endswith(".json"):
            decl = json.loads(data)
            for key in ("shapes", "collision_shapes"):
                if key in decl:
                    decl[key] = fix_shapes(decl[key])
            # The light tunnel's output lights by its state (LightP2PTunnelPart): `light` 0..15 and `state_light`.
            if rel.endswith("/light_p2p_tunnel.json"):
                decl.setdefault("state_properties", {})["light"] = {"type": "enum", "values": [str(i) for i in range(16)]}
                decl.setdefault("default_state", {})["light"] = "0"
                decl["state_light"] = {"properties": ["light"], "values": {str(i): i for i in range(16)}, "default": 0}
            # The spatial anchor shows whether it is powered (spatial.rs sets it): its `_on` model.
            if rel.endswith("/spatial_anchor.json"):
                decl.setdefault("state_properties", {})["powered"] = {"type": "boolean"}
            # The annihilation plane takes enchantments in a table and an anvil (its item's `minecraft:enchantable`; the
            # `minecraft:enchantable/*` tags list it): AnnihilationPlaneItem.isEnchantable.
            if rel.endswith("/annihilation_plane.json"):
                decl.setdefault("components", {})["minecraft:enchantable"] = {"value": 10}
            # fixE: the light detector lets the sky's light through (LightDetectorBlock: no occlusion, skylight passes
            # down), so its own block has the sky's full 15 and noon sun gives AE2's 9. Sunrise takes a shulker box
            # template's dry skylight answer (none down: 14 here); a beacon's follows the declared outline, a partial
            # one passing it. The beacon's light 15 is cleared; `facing` (shulker's own) is declared, placed on the
            # clicked face by misc_blocks::placement.
            if rel.endswith("/light_detector.json") and decl.get("template") == "minecraft:shulker_box":
                decl["template"] = "minecraft:beacon"
                decl.setdefault("properties", {}).update({"light": 0, "sounds": "minecraft:shulker_box", "explosion_resistance": 2.0})
                decl.setdefault("state_properties", {})["facing"] = {"type": "enum", "values": ["down", "up", "north", "south", "west", "east"]}
                decl.setdefault("default_state", {})["facing"] = "up"
            if make_cables.is_cable_block(os.path.basename(rel)[:-5]):
                make_cables.add_core_state(decl)
            if decl.get("template") in BE_TEMPLATES:
                be_blocks.append((os.path.basename(rel)[:-5], decl["template"]))
            data = json.dumps(decl, indent=2).encode()
        if rel.startswith(f"assets/{ID}/items/") and CELL_ITEM(os.path.basename(rel)):
            data = cell_tint(data)
        out[rel] = data
    # The stand-in a terminal's shift-click leaves when the whole stack went into the network (storage.rs MOVED).
    out[f"data/{ID}/sunrise/item/hz_moved.json"] = (json.dumps({"template": "minecraft:barrier", "max_stack_size": 1}, indent=2) + "\n").encode()
    lang = f"assets/{ID}/lang/en_us.json"
    if lang in out:
        table = json.loads(out[lang].decode("utf-8", "replace"))
        table["item.ae2.hz_moved"] = "Moved"
        out[lang] = json.dumps(table, indent=2, ensure_ascii=False).encode()
    # The cables as AE2 bakes them (the content's `hz_cable` models had a core in every straight run and end caps).
    make_cables.write(out)
    # Files this port wrote by hand (the spatial anchor's powered look, its menus' slot counts): tools/overrides/.
    over = os.path.join(HERE, "overrides")
    for dirpath, _, names in os.walk(over):
        for n in names:
            full = os.path.join(dirpath, n)
            out[sunrise_path(os.path.relpath(full, over).replace(os.sep, "/"))] = open(full, "rb").read()
    # fixG: the back-lying parts' front-mounted states.
    for name in FRONT_MOUNTED:
        bs, decl = f"assets/{ID}/blockstates/{name}.json", f"data/{ID}/sunrise/block/{name}.json"
        if bs in out and decl in out:
            out[bs] = front_mounted_blockstate(out[bs])
            out[decl] = front_mounted_decl(out[decl])
        else:
            print("fixG: no blockstate or declaration for", name)
    # The meteorites: AE2's structure and structure set, the structure's type being this port's own (src/meteor.rs).
    out["data/ae2/worldgen/structure/meteorite.json"] = (json.dumps({"type": "ae2:meteorite", "biomes": "#ae2:has_meteorites", "spawn_overrides": {}, "step": "top_layer_modification"}, indent=2) + "\n").encode()
    out["data/ae2/worldgen/structure_set/meteorite.json"] = (json.dumps({"placement": {"type": "minecraft:random_spread", "salt": 124895654, "separation": 8, "spacing": 32}, "structures": [{"structure": "ae2:meteorite", "weight": 1}]}, indent=2) + "\n").encode()
    # The spatial storage dimension's biome (the level itself is registered in code: src/spatial.rs).
    # In 26.3's biome form (AE2's jar has the 1.21 one, which Sunrise ed3ac19 refuses): colours are attributes, carvers a list.
    out["data/ae2/worldgen/biome/spatial_storage.json"] = (json.dumps({
        "attributes": {
            "minecraft:gameplay/natural_mob_spawns": {"argument": {"spawn_costs": {}, "spawns_by_category": {k: [] for k in (
                "ambient", "axolotls", "creature", "misc", "monster", "underground_water_creature", "water_ambient", "water_creature")}},
                "modifier": "overlay"},
            "minecraft:visual/sky_color": "#111111", "minecraft:visual/fog_color": "#000000", "minecraft:visual/water_fog_color": "#050533"},
        "carvers": [], "downfall": 0.0, "effects": {"water_color": "#3f76e4"}, "features": [], "has_precipitation": False,
        "temperature": 0.5}, indent=2) + "\n").encode()
    for tree in ("assets", "data"):
        shutil.rmtree(os.path.join(ROOT, tree), ignore_errors=True)
    for rel, data in out.items():
        path = os.path.join(ROOT, *rel.split("/"))
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "wb") as f:
            f.write(data)
    be_blocks.sort()
    lines = [
        "// Written by tools/make_fork_content.py from the content's block declarations: don't edit.",
        "",
        "/// Every AE2 block whose template has a vanilla block entity, and that template.",
        "pub(crate) const BE_BLOCKS: &[(&str, &str)] = &[",
    ]
    lines += [f"    ({json.dumps(n)}, {json.dumps(t)})," for n, t in be_blocks]
    lines.append("];")
    with open(os.path.join(ROOT, "src", "blocks_gen.rs"), "w", encoding="utf8", newline="\n") as f:
        f.write("\n".join(lines) + "\n")
    print(len(out), "files written under", ROOT, ";", len(be_blocks), "blocks with a block entity template")
    # JEI's categories and the Fluix Researcher's trades come from their own scripts, into the trees just rewritten.
    import subprocess
    for script in ("make_jei.py", "make_trades.py"):
        subprocess.run([sys.executable, os.path.join(HERE, script)], check=True)


def commands_menu():
    """The `/ae2` commands menu's pages (`../../tools/commands-menu/ae2.json`), written into assets/ after the wipe."""
    import importlib.util
    path = os.path.join(ROOT, "..", "tools", "commands-menu", "pages.py")
    spec = importlib.util.spec_from_file_location("commands_menu_pages", path)
    pages = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(pages)
    pages.install(ROOT, "ae2")


if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else SOURCE)
    commands_menu()
