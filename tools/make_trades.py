"""Writes the Fluix Researcher's trades (AE2's InitVillager) as 26.3's data: data/ae2/villager_trade/fluix_researcher/
<level>/<item>.json and data/ae2/trade_set/fluix_researcher/level_<n>.json (two trades a level, as Villager.updateTrades).
Upstream's researcher_trades.rs, as files. Run: python tools/make_trades.py"""
import json, pathlib
ROOT = pathlib.Path(__file__).resolve().parent.parent / "data/ae2"
BUY = [(1, "ae2:certus_quartz_crystal", 3, 4, 10), (1, "ae2:meteorite_compass", 2, 1, 5), (2, "ae2:sky_stone_block", 5, 8, 20),
       (3, "ae2:fluix_crystal", 5, 4, 14), (4, "ae2:calculation_processor_press", 10, 1, 20), (4, "ae2:engineering_processor_press", 10, 1, 20),
       (4, "ae2:logic_processor_press", 10, 1, 20), (4, "ae2:silicon_press", 10, 1, 20), (5, "minecraft:slime_ball", 8, 5, 12)]
SELL = [(2, "ae2:charged_certus_quartz_crystal", 3, 10, 15), (2, "ae2:silicon", 5, 8, 13), (3, "ae2:quartz_glass", 2, 10, 10), (4, "ae2:matter_ball", 5, 8, 12)]
levels = {n: [] for n in range(1, 6)}
def add(level, item, trade):
    path = item.split(":")[1]
    f = ROOT / f"villager_trade/fluix_researcher/{level}/{path}.json"
    f.parent.mkdir(parents=True, exist_ok=True)
    f.write_text(json.dumps(trade, indent=2) + "\n", encoding="utf-8")
    levels[level].append(f"ae2:fluix_researcher/{level}/{path}")
for level, item, emeralds, count, xp in BUY:
    add(level, item, {"wants": {"id": "minecraft:emerald", "count": emeralds}, "gives": {"id": item, "count": count}, "max_uses": 12, "xp": xp, "reputation_discount": 0.05})
for level, item, count, uses, xp in SELL:
    add(level, item, {"wants": {"id": item, "count": count}, "gives": {"id": "minecraft:emerald", "count": 1}, "max_uses": uses, "xp": xp, "reputation_discount": 0.05})
for level, trades in levels.items():
    f = ROOT / f"trade_set/fluix_researcher/level_{level}.json"
    f.parent.mkdir(parents=True, exist_ok=True)
    f.write_text(json.dumps({"amount": 2, "random_sequence": f"ae2:trade_set/fluix_researcher/level_{level}", "trades": trades}, indent=2) + "\n", encoding="utf-8")
print("trades written")
