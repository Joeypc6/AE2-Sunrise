---
navigation:
  title: "testmeteorites"
  parent: index.md
  position: 8
  icon: ae2:sky_stone_block
level: 4
---
# testmeteorites

**Uso:** `/ae2 testmeteorites [radius]`

Lista los meteoritos calculados dentro del radio (1000 bloques por defecto) que aún no se han colocado, con dónde caerán y a qué distancia están.

<GameScene zoom="4" interactive={true}>
  <Block id="ae2:sky_stone_block" x="0" y="0" z="0" />
  <Block id="ae2:sky_stone_block" x="1" y="0" z="0" />
  <Block id="ae2:sky_stone_block" x="0" y="0" z="1" />
  <Block id="ae2:sky_stone_block" x="1" y="0" z="1" />
  <Block id="ae2:mysterious_cube" x="0" y="1" z="0" />
  <IsometricCamera yaw="195" pitch="30" />
</GameScene>
