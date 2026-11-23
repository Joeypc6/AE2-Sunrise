---
navigation:
  title: "channelmode"
  parent: index.md
  position: 6
  icon: ae2:fluix_smart_cable
level: 4
---
# channelmode

**Uso:** `/ae2 channelmode [infinite|default|x2|x3|x4]`

Sin modo, muestra el modo de canales. Con uno, lo fija para todas las redes y lo guarda en la configuración:

- `default`: 8 canales por cable, 32 por cable denso;
- `x2`, `x3`, `x4`: el doble, triple o cuádruple;
- `infinite`: sin límite de canales.

Todas las redes recalculan sus canales al momento.

<GameScene zoom="4" interactive={true}>
  <Block id="ae2:controller" x="0" y="0" z="0" />
  <Block id="ae2:drive" x="1" y="0" z="0" />
  <Block id="ae2:drive" x="-1" y="0" z="0" />
  <IsometricCamera yaw="195" pitch="30" />
</GameScene>
