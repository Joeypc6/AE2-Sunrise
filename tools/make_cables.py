"""AE2's cables as block models: what CableBuilder and CableBusBakedModel (appeng.client.render.cablebus) bake at run time,
written out as the cable blocks' multipart blockstates and their `hz_cable/*` models.

How AE2 draws a cable block (CableBusBakedModel.buildCableModel):
- a straight run (exactly two opposite connections of the cable's own type, no part on it) is one cube the length of the
  block, no core: glass draws its sides only (no ends), with the texture's own UVs, so a line of glass cables is one
  tube; covered, smart and dense map a 5-pixel strip of their texture along the cable (`setStraightCableUVs`, the UV
  turned half a turn on some faces) and keep their end faces;
- anything else is the core (`part/cable/core/<glass|covered|dense_smart>/<colour>`, 4, 6 or 10 pixels wide) and an arm
  to each connection, without the face at the block's edge: glass 6..10 wide from the core out, covered and smart
  6..10 from 5 out (the core takes the rest), dense 4..12 from 5 out, each with its type's texture and the face's
  standard UVs (`CubeBuilder.getStandardUv`);
- smart cables add their channel lights over the base: `channels_0N` tinted with the colour's dark variant and
  `channels_1N` with its light one, drawn full bright (here one baked overlay texture per colour and light count,
  `textures/block/hz_cable/<kind>_<colour>_ch<N>`, and `light_emission` 15).

Each model is written per side and axis in world space (no blockstate rotation, no uvlock), with each face's UV as
CubeBuilder emits it: its vertices carry the same texture corners as AE2's quads (`uv` plus `rotation`), and a face on
the block's edge culls against that side as Fabric's `QuadEmitter.square` does.

The cable bus (`bus.rs`) draws its cell's cable from these same models, piece by piece (registered as standalone models,
drawn as cut-out terrain): the core while a part sits on it. fixE: a smart cable's pieces there have their lights off
too, and its lit lines are the plain cables' `hz_cable/lit/*` overlays (below), drawn over them side by side.
A cable block's `x_core` does the same for the block's state (the fallback draw).

fixB: a plain smart cable block's own model has every light off (`_ch0`): a count per side as block state would be
9^6 x 64 x 2 states a block, past Sunrise's 2^20 for the whole game. Its block entity (`src/cable_lights.rs`) draws the
lit lines over it, side by side, from the `hz_cable/lit/<kind>_<piece>_<tex>` overlays: the piece's light element
alone, a hair further out than the dark one, its faces tinted (the odd lights `channels_0N` with the colour's dark
variant, the even ones `channels_1N` with its light one). The guide's scenes give their smart cables that block entity
with the lights they had as `x_lights`.
"""

import io
import json

from PIL import Image

ID = "ae2"
COLORS = ["white", "orange", "magenta", "light_blue", "yellow", "lime", "pink", "gray", "light_gray", "cyan", "purple",
          "blue", "brown", "green", "red", "black", "fluix"]
# AEColor's dark and light variants (blackVariant, whiteVariant): the smart cables' odd and even channel lights.
TINTS = {
    "white": (12500670, 16448250), "orange": (16357177, 16047811), "magenta": (8527490, 12949704),
    "light_blue": (6458827, 14219007), "yellow": (16775082, 16777192), "lime": (8191818, 15202263),
    "pink": (14454197, 16244459), "gray": (8158332, 13224393), "light_gray": (10329501, 15724527),
    "cyan": (3120037, 11460084), "purple": (8532146, 13083596), "blue": (2959776, 14542591),
    "brown": (7491125, 14734024), "green": (4562977, 14938851), "red": (10813481, 16770797),
    "black": (2829099, 8684676), "fluix": (1778500, 14138348),
}
# The block's name part → AE2's cable type (AECableType) as these models name it.
KINDS = {"glass": "glass", "covered": "covered", "smart": "smart", "covered_dense": "dense_covered",
         "smart_dense": "dense_smart"}
CORE_TEX = {"glass": "glass", "covered": "covered", "smart": "covered", "dense_covered": "dense_smart",
            "dense_smart": "dense_smart"}
SMART = ("smart", "dense_smart")
# fixE: only the lights-off pieces (`_ch0`): the cable bus draws its smart arms' lit lines as the plain cables do, with the
# tinted `hz_cable/lit/*` overlays (cable_lights.rs), so the baked `_ch1`..`_ch8` pieces and textures are gone.
LIGHTS = 1
# fixB: the light textures a lit overlay draws (odd 1..4, even 1..4; 0 is the block model's own dark lines).
LIT_TEXTURES = ["01", "02", "03", "04", "11", "12", "13", "14"]

# Direction in AE2's (Minecraft's) order; an EnumMap's first key is the lowest.
DIRS = ["down", "up", "north", "south", "west", "east"]
OPP = {"down": "up", "up": "down", "north": "south", "south": "north", "west": "east", "east": "west"}
# Each axis's straight run, named by the facing AE2 builds it with (the first of its two connections).
AXES = {"y": ("down", "up"), "z": ("north", "south"), "x": ("west", "east")}


def tex_name(color):
    return "transparent" if color == "fluix" else color


def num(x):
    """A coordinate as the models write it: 6 not 6.0 or -0.0, -0.01 not -0.010000000000000009."""
    x = round(x, 4) + 0.0
    return int(x) if x == int(x) else x


class Cube:
    """CubeBuilder: the faces of each `add` with its texture, custom UVs, UV turns and flips."""

    def __init__(self):
        self.draw = list(DIRS)
        self.custom = {}
        self.rot = {}
        self.flip_u = set()
        self.flip_v = set()
        self.elements = []

    def add(self, tex, x1, y1, z1, x2, y2, z2, inflate=0.0, light=False):
        faces = {}
        for f in self.draw:
            faces[f] = self.face(tex, f, x1, y1, z1, x2, y2, z2)
        if not faces:
            return
        e = {"from": [num(x1 - inflate), num(y1 - inflate), num(z1 - inflate)],
             "to": [num(x2 + inflate), num(y2 + inflate), num(z2 + inflate)], "faces": faces}
        if inflate:
            # Coplanar with the base face but a hair out: its edge faces no longer touch the block's edge.
            for f, d in faces.items():
                d.pop("cullface", None)
        if light:
            e["light_emission"] = 15
        self.elements.append(e)

    def face(self, tex, f, x1, y1, z1, x2, y2, z2):
        if f in self.custom:
            u1, v1, u2, v2 = self.custom[f]
        else:
            # getStandardUv
            if f in ("down", "up"):
                v1, v2 = z1, z2
            else:
                v1, v2 = 16 - y1, 16 - y2
            u1, u2 = {"down": (x1, x2), "up": (x1, x2), "south": (x1, x2), "north": (16 - x2, 16 - x1),
                      "west": (z1, z2), "east": (16 - z2, 16 - z1)}[f]
        if f in self.flip_u:
            u1, u2 = u2, u1
        if f in self.flip_v:
            v1, v2 = v2, v1
        # setFaceUV: vertex (i - r) % 4 takes corner i of (u1,v1) (u1,v2) (u2,v2) (u2,v1) on top and bottom faces and
        # (u1,v2) (u1,v1) (u2,v1) (u2,v2) on the sides; a model face's vertex j takes corner (j + rotation/90) % 4 of
        # (u0,v0) (u0,v1) (u1,v1) (u1,v0), its vertices in the same order as QuadEmitter.square's.
        uv = [u1, v1, u2, v2] if f in ("down", "up") else [u1, v2, u2, v1]
        d = {"texture": tex, "uv": [num(c) for c in uv]}
        r = self.rot.get(f, 0)
        if r:
            d["rotation"] = 90 * r
        depth = {"down": y1, "up": 16 - y2, "north": z1, "south": 16 - z2, "west": x1, "east": 16 - x2}[f]
        if abs(depth) < 1e-4:
            d["cullface"] = f
        return d


# ---- CableBuilder's pieces (the cube for each side or axis)

def glass_arm(f):
    return {"down": (6, 0, 6, 10, 6, 10), "east": (10, 6, 6, 16, 10, 10), "north": (6, 6, 0, 10, 10, 6),
            "south": (6, 6, 10, 10, 10, 16), "up": (6, 10, 6, 10, 16, 10), "west": (0, 6, 6, 6, 10, 10)}[f]


def covered_arm(f):  # addCoveredCableSizedCube
    return {"down": (6, 0, 6, 10, 5, 10), "east": (11, 6, 6, 16, 10, 10), "north": (6, 6, 0, 10, 10, 5),
            "south": (6, 6, 11, 10, 10, 16), "up": (6, 11, 6, 10, 16, 10), "west": (0, 6, 6, 5, 10, 10)}[f]


def dense_arm(f):  # addDenseCableSizedCube
    return {"down": (4, 0, 4, 12, 5, 12), "east": (11, 4, 4, 16, 12, 12), "north": (4, 4, 0, 12, 12, 5),
            "south": (4, 4, 11, 12, 12, 16), "up": (4, 11, 4, 12, 16, 12), "west": (0, 4, 4, 5, 12, 12)}[f]


def straight_uvs(c, f, x, y):  # setStraightCableUVs
    if f in ("down", "up"):
        for s in ("north", "east", "south", "west"):
            c.custom[s] = (x, 0, y, x)
    elif f in ("east", "west"):
        for s in ("up", "down", "north", "south"):
            c.custom[s] = (0, x, x, y)
    else:
        for s in ("up", "down"):
            c.custom[s] = (x, 0, y, x)
        for s in ("east", "west"):
            c.custom[s] = (0, x, x, y)


def straight_cube(c, f, lo, hi, ext=0.0):
    """addStraightCoveredCableSizedCube (lo 5, hi 11) and addStraightDenseCableSizedCube (3, 13, ends 0.01 out)."""
    if f in ("down", "up"):
        c.rot = {"east": 2}
        return (lo, -ext, lo, hi, 16 + ext, hi)
    if f in ("east", "west"):
        c.rot = {"south": 2, "north": 2}
        return (-ext, lo, lo, 16 + ext, hi, hi)
    c.rot = {"east": 2, "west": 2}
    return (lo, lo, -ext, hi, hi, 16 + ext)


def straight(kind, f):
    c = Cube()
    if kind == "glass":  # addStraightGlassConnection
        c.draw = [d for d in DIRS if d not in (f, OPP[f])]
        box = {"y": (6, 0, 6, 10, 16, 10), "x": (0, 6, 6, 16, 10, 10), "z": (6, 6, 0, 10, 10, 16)}
        c.add("#base", *box[axis_of(f)])
        return c.elements
    if kind == "smart":  # addStraightSmartConnection
        if f in ("down", "up"):
            c.flip_u = {"north"}
        elif f in ("east", "west"):
            c.flip_v = {"down"}
    if kind == "dense_smart":  # addStraightDenseSmartConnection
        if f == "down":
            c.flip_u, c.flip_v = {"north"}, {"down"}
        elif f == "north":
            c.flip_u = {"north"}
        elif f == "west":
            c.flip_v, c.flip_u = {"down"}, {"east"}
    dense = kind.startswith("dense")
    straight_uvs(c, f, *((3, 13) if dense else (5, 11)))
    box = straight_cube(c, f, *((3, 13, 0.01) if dense else (5, 11)))
    c.add("#base", *box)
    if kind in SMART:
        c.add("#channels", *box, inflate=0.01, light=True)
    return c.elements


def core(kind):  # addCableCore
    c = Cube()
    lo, hi = {"glass": (6, 10), "covered": (5, 11), "smart": (5, 11), "dense_covered": (3, 13), "dense_smart": (3, 13)}[kind]
    c.add("#core", lo, lo, lo, hi, hi, hi)
    return c.elements


def arm(kind, f):
    """The arm to a cable of the same type (`cableBusAdjacent`: no collar): addGlassConnection, addCoveredConnection,
    addSmartConnection, addDenseCoveredConnection, addDenseSmartConnection."""
    c = Cube()
    c.draw = [d for d in DIRS if d != f]
    if kind == "glass":
        c.add("#base", *glass_arm(f))
    elif kind == "covered":
        c.add("#base", *covered_arm(f))
    elif kind == "smart":
        c.flip_u, c.flip_v = {"down": ({"east", "north"}, set()), "east": (set(), {"down"}), "north": (set(), set()),
                              "south": ({"north"}, set()), "up": ({"east", "north"}, {"down"}),
                              "west": ({"east"}, {"down"})}[f]
        c.add("#base", *covered_arm(f))
        c.add("#channels", *covered_arm(f), inflate=0.01, light=True)
    elif kind == "dense_covered":
        c.add("#base", *dense_arm(f))
    else:
        if f in ("down", "up"):
            c.flip_u = {"north", "east"}
        elif f in ("east", "west"):
            c.flip_v = {"down"}
        c.add("#base", *dense_arm(f))
        c.add("#channels", *dense_arm(f), inflate=0.01, light=True)
    return c.elements


def axis_of(f):
    return next(a for a, pair in AXES.items() if f in pair)


# ---- The smart cables' lights

def overlay(chan_odd, chan_even, color):
    """The odd lights tinted dark and the even ones light (CubeBuilder.setColorRGB multiplies the texture)."""
    dark, light = TINTS[color]
    odd, even = Image.open(io.BytesIO(chan_odd)).convert("RGBA"), Image.open(io.BytesIO(chan_even)).convert("RGBA")
    out = Image.new("RGBA", odd.size, (0, 0, 0, 0))
    px, po, pe = out.load(), odd.load(), even.load()
    for y in range(odd.size[1]):
        for x in range(odd.size[0]):
            for p, t in ((po[x, y], dark), (pe[x, y], light)):
                if p[3]:
                    px[x, y] = (p[0] * (t >> 16 & 255) // 255, p[1] * (t >> 8 & 255) // 255, p[2] * (t & 255) // 255, p[3])
    buf = io.BytesIO()
    out.save(buf, "PNG")
    return buf.getvalue()


def light_textures(n):
    """SmartCableTextures: the odd texture's index and the even one's for `n` lights (n channels, or n fours)."""
    return f"0{min(n, 4)}", (f"1{n - 4}" if n >= 5 else "10")


# ---- Blockstates

def multipart(kind, color):
    smart = kind in SMART
    name = f"{ID}:block/hz_cable/{kind}_{color}"

    def not_straight(axis):
        a, b = AXES[axis]
        return {"OR": [{a: "false"}, {b: "false"}] + [{d: "true"} for d in DIRS if d not in (a, b)] + [{"x_core": "true"}]}

    parts = []
    # fixB: a smart cable's lights off (its block entity draws the lit ones, cable_lights.rs).
    ch = "_ch0" if smart else ""
    for axis, (a, b) in AXES.items():
        when = {d: "true" if d in (a, b) else "false" for d in DIRS}
        when["x_core"] = "false"
        parts.append({"when": when, "apply": {"model": f"{name}_straight_{axis}{ch}"}})
    for d in DIRS:
        parts.append({"when": {"AND": [{d: "true"}, not_straight(axis_of(d))]}, "apply": {"model": f"{name}_arm_{d}{ch}"}})
    parts.append({"when": {"AND": [not_straight(a) for a in AXES]}, "apply": {"model": f"{name}_core"}})
    return {"multipart": parts}


def lit(elements):
    """fixB: a piece's light element alone, 0.01 further out than the block model's dark one (drawn over it), its
    faces tinted (cable_lights.rs passes the colour's dark or light variant)."""
    out = []
    for e in elements:
        if not e.get("light_emission"):
            continue
        e = json.loads(json.dumps(e))
        e["from"] = [num(c - 0.01) for c in e["from"]]
        e["to"] = [num(c + 0.01) for c in e["to"]]
        for face in e["faces"].values():
            face["tintindex"] = 0
        out.append(e)
    return out


def guide_scene(data):
    """fixB: a guide scene's smart cables lose `x_lights` (no longer a state) and get their block entity with that many
    lights on every side (cable_lights.rs `lights`: 4 bits a side). None when the scene has none."""
    scene = json.loads(data)
    changed = False
    for b in scene.get("blocks", []):
        if not (isinstance(b, list) and len(b) >= 5 and isinstance(b[4], dict) and "x_lights" in b[4]):
            continue
        n = min(int(b[4].pop("x_lights")), 8)
        changed = True
        # No lights: the block model's dark lines are all there is.
        if len(b) == 5 and n > 0:
            packed = sum(n << (4 * s) for s in range(6))
            b.append({"entity": f"{ID}:smart_cable", "nbt": "{lights:%d}" % packed})
    return (json.dumps(scene, indent=2) + "\n").encode() if changed else None


def model(textures, elements):
    return {"textures": textures, "elements": elements}


def dump(obj):
    return (json.dumps(obj, indent=2) + "\n").encode()


def write(out):
    """Replaces the content's cable models, their blockstates and the light overlays in `out` (path → bytes)."""
    for rel in [r for r in out if r.startswith((f"assets/{ID}/models/block/hz_cable/", f"assets/{ID}/textures/block/hz_cable/"))]:
        del out[rel]
    tex = f"assets/{ID}/textures/block/part/cable"
    for kind in KINDS.values():
        for color in COLORS:
            t = tex_name(color)
            base = f"{ID}:block/part/cable/{kind}/{t}"
            core_tex = f"{ID}:block/part/cable/core/{CORE_TEX[kind]}/{t}"
            pre = f"assets/{ID}/models/block/hz_cable/{kind}_{color}"
            out[f"{pre}_core.json"] = dump(model({"core": core_tex, "particle": core_tex}, core(kind)))
            for n in range(LIGHTS) if kind in SMART else [None]:
                ch = "" if n is None else f"_ch{n}"
                textures = {"base": base, "particle": base}
                if n is not None:
                    odd, even = light_textures(n)
                    lights = f"textures/block/hz_cable/{kind}_{color}_ch{n}"
                    out[f"assets/{ID}/{lights}.png"] = overlay(out[f"{tex}/{kind}/channels_{odd}.png"], out[f"{tex}/{kind}/channels_{even}.png"], color)
                    textures["channels"] = f"{ID}:block/hz_cable/{kind}_{color}_ch{n}"
                for axis, (f, _) in AXES.items():
                    out[f"{pre}_straight_{axis}{ch}.json"] = dump(model(textures, straight(kind, f)))
                for d in DIRS:
                    out[f"{pre}_arm_{d}{ch}.json"] = dump(model(textures, arm(kind, d)))
    for color in COLORS:
        for part, kind in KINDS.items():
            out[f"assets/{ID}/blockstates/{color}_{part}_cable.json"] = dump(multipart(kind, color))
    # fixB: the plain smart cables' lit overlays (one per kind, piece and light texture; tinted when drawn).
    for kind in SMART:
        for t in LIT_TEXTURES:
            tex = f"{ID}:block/part/cable/{kind}/channels_{t}"
            textures = {"channels": tex, "particle": tex}
            pre = f"assets/{ID}/models/block/hz_cable/lit/{kind}"
            for axis, (f, _) in AXES.items():
                out[f"{pre}_straight_{axis}_{t}.json"] = dump(model(textures, lit(straight(kind, f))))
            for d in DIRS:
                out[f"{pre}_arm_{d}_{t}.json"] = dump(model(textures, lit(arm(kind, d))))
    fiber_models(out)  # fixE
    # fixB: the guide's scenes light their smart cables through the block entity.
    for rel in [r for r in out if r.startswith(f"assets/{ID}/ae2guide/") and r.endswith(".json")]:
        fixed = guide_scene(out[rel])
        if fixed is not None:
            out[rel] = fixed


# fixE: the quartz fiber on a cable bus side (QuartzFiberPart.getBoxes: 6, 6, 10 to 10, 10, 16 facing south, from the
# cable's core out to the face), one model a side in world space, as bus.rs `part_boxes` places it.
FIBER_BOX = (6, 6, 10, 10, 10, 16)


def on_side(box, side):
    """bus.rs `on_side`: a south-facing box (z 16 the outer face) turned to `side`, in pixels."""
    a0, b0, u0, a1, b1, u1 = box
    if DIRS.index(side) % 2 == 0:
        u0, u1 = 16 - u1, 16 - u0
    if side in ("down", "up"):
        return (a0, u0, b0, a1, u1, b1)
    if side in ("north", "south"):
        return (a0, b0, u0, a1, b1, u1)
    return (u0, b0, a0, u1, b1, a1)


def fiber_models(out):
    """fixE: `hz_part/quartz_fiber_bus_<side>`: AE2's quartz fiber rod (`part/quartz_fiber`) from the core to the face.
    fixG: AE2's part model itself (6, 6, 0 to 10, 10, 6 for a part on the north side, every face with the UVs the game
    gives a face without any), turned to each side as CableBusBakedModel turns a part's quads (QuadRotator with
    BlockOrientation.get(side, 0)), each face's UVs and turn carried with it; no cull faces (the part model has none).
    fixE's own cube used CubeBuilder's UVs and culled its outer face."""
    tex = f"{ID}:block/part/quartz_fiber"
    for side in DIRS:
        elements = [rotate_element(e, side) for e in AE2_FIBER]
        out[f"assets/{ID}/models/block/hz_part/quartz_fiber_bus_{side}.json"] = dump(model({"fiber": tex, "particle": tex}, elements))


# fixG: AE2's `models/part/quartz_fiber.json` (appliedenergistics2-fabric 15.4.12), its part on the north side.
AE2_FIBER = [{"from": [6, 6, 0], "to": [10, 10, 6], "faces": {f: {"texture": "#fiber"} for f in DIRS}}]

# BlockOrientation.get(side, 0): (angleX, angleY, angleZ); its quaternion is rotateYXZ(-y, -x, -z).
ORIENT = {"down": (90, 0, 0), "up": (270, 0, 180), "north": (0, 0, 0), "south": (0, 180, 0), "west": (0, 270, 0),
          "east": (0, 90, 0)}
NORMALS = {"down": (0, -1, 0), "up": (0, 1, 0), "north": (0, 0, -1), "south": (0, 0, 1), "west": (-1, 0, 0),
           "east": (1, 0, 0)}


def _rot(axis, deg):
    c, s = [(1, 0), (0, 1), (-1, 0), (0, -1)][(deg // 90) % 4]
    if axis == "x":
        return [[1, 0, 0], [0, c, -s], [0, s, c]]
    if axis == "y":
        return [[c, 0, s], [0, 1, 0], [-s, 0, c]]
    return [[c, -s, 0], [s, c, 0], [0, 0, 1]]


def _mul(a, b):
    return [[sum(a[i][k] * b[k][j] for k in range(3)) for j in range(3)] for i in range(3)]


def orientation_matrix(side):
    """Quaternionf().rotateYXZ(-y, -x, -z) as a matrix (exact: every angle a multiple of 90)."""
    ax, ay, az = ORIENT[side]
    return _mul(_mul(_rot("y", -ay), _rot("x", -ax)), _rot("z", -az))


def _apply(m, v):
    return tuple(sum(m[i][k] * v[k] for k in range(3)) for i in range(3))


def face_corners(f, lo, hi):
    """FaceInfo: a face's four vertices, in the order the game bakes them."""
    (x1, y1, z1), (x2, y2, z2) = lo, hi
    return {
        "down": [(x1, y1, z2), (x1, y1, z1), (x2, y1, z1), (x2, y1, z2)],
        "up": [(x1, y2, z1), (x1, y2, z2), (x2, y2, z2), (x2, y2, z1)],
        "north": [(x2, y2, z1), (x2, y1, z1), (x1, y1, z1), (x1, y2, z1)],
        "south": [(x1, y2, z2), (x1, y1, z2), (x2, y1, z2), (x2, y2, z2)],
        "west": [(x1, y2, z1), (x1, y1, z1), (x1, y1, z2), (x1, y2, z2)],
        "east": [(x2, y2, z2), (x2, y1, z2), (x2, y1, z1), (x2, y2, z1)],
    }[f]


def default_uv(f, lo, hi):
    """BlockElement's UVs for a face that names none."""
    (x1, y1, z1), (x2, y2, z2) = lo, hi
    return {"down": [x1, 16 - z2, x2, 16 - z1], "up": [x1, z1, x2, z2], "north": [16 - x2, 16 - y2, 16 - x1, 16 - y1],
            "south": [x1, 16 - y2, x2, 16 - y1], "west": [z1, 16 - y2, z2, 16 - y1],
            "east": [16 - z2, 16 - y2, 16 - z1, 16 - y1]}[f]


def vertex_uvs(uv, rotation):
    """BlockFaceUV: vertex i's (u, v), the corner (i + rotation / 90) % 4 of (u1,v1) (u1,v2) (u2,v2) (u2,v1)."""
    u1, v1, u2, v2 = uv
    corners = [(u1, v1), (u1, v2), (u2, v2), (u2, v1)]
    return [corners[(i + rotation // 90) % 4] for i in range(4)]


def rotate_element(e, side):
    """fixG: a part-model element (its part on the north side) turned onto `side` in world space: the box turned
    about the block's centre, and each face moved to the face it lands on with the UV and turn that give each of its
    vertices the texture point it had (QuadRotator moves the vertices and keeps their UVs)."""
    m = orientation_matrix(side)
    lo, hi = e["from"], e["to"]
    turn = lambda p: tuple(c + 8 for c in _apply(m, tuple(q - 8 for q in p)))
    a, b = turn(lo), turn(hi)
    nlo = tuple(min(a[i], b[i]) for i in range(3))
    nhi = tuple(max(a[i], b[i]) for i in range(3))
    faces = {}
    for f, face in e["faces"].items():
        uv = face.get("uv", default_uv(f, lo, hi))
        texel = {turn(p): t for p, t in zip(face_corners(f, lo, hi), vertex_uvs(uv, face.get("rotation", 0)))}
        nf = next(d for d, n in NORMALS.items() if n == _apply(m, NORMALS[f]))
        want = [texel[p] for p in face_corners(nf, nlo, nhi)]
        for r in (0, 90, 180, 270):
            # Vertex k takes corner (k + r/90) % 4: corners 0 and 2 name the UV, 1 and 3 must agree.
            k0, k2 = (4 - r // 90) % 4, (6 - r // 90) % 4
            nuv = [want[k0][0], want[k0][1], want[k2][0], want[k2][1]]
            if vertex_uvs(nuv, r) == want:
                d = {"texture": face["texture"], "uv": [num(c) for c in nuv]}
                if r:
                    d["rotation"] = r
                faces[nf] = d
                break
        else:
            raise ValueError(f"no UV turn for {f} on {side}")
    return {"from": [num(c) for c in nlo], "to": [num(c) for c in nhi], "faces": {f: faces[f] for f in DIRS if f in faces}}


def is_cable_block(name):
    """`fluix_glass_cable`, `red_smart_dense_cable`: a cable block (not the bus's `hz_fcable_*` stand-ins)."""
    return any(name == f"{c}_{p}_cable" for c in COLORS for p in KINDS)


def add_core_state(decl):
    """A cable block's `x_core` (bus.rs sets it while a part sits on the cable). fixB: no `x_lights` (a smart cable's
    lights are its block entity's, cable_lights.rs)."""
    decl.setdefault("state_properties", {})["x_core"] = {"type": "boolean"}
    decl.setdefault("default_state", {})["x_core"] = "false"
    decl["state_properties"].pop("x_lights", None)
    decl.get("default_state", {}).pop("x_lights", None)
    return decl
