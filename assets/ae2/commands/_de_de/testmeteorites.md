---
navigation:
  title: "testmeteorites"
  parent: index.md
  position: 8
  icon: ae2:sky_stone_block
level: 4
---
# testmeteorites

**Verwendung:** `/ae2 testmeteorites [radius]`

Listet die im Radius (standardmäßig 1000 Blöcke) berechneten, noch nicht platzierten Meteoriten auf, mit Einschlagsort und Entfernung.

<GameScene zoom="4" interactive={true}>
  <Block id="ae2:sky_stone_block" x="0" y="0" z="0" />
  <Block id="ae2:sky_stone_block" x="1" y="0" z="0" />
  <Block id="ae2:sky_stone_block" x="0" y="0" z="1" />
  <Block id="ae2:sky_stone_block" x="1" y="0" z="1" />
  <Block id="ae2:mysterious_cube" x="0" y="1" z="0" />
  <IsometricCamera yaw="195" pitch="30" />
</GameScene>
