---
navigation:
  title: "tickmonitor"
  parent: index.md
  position: 5
  icon: minecraft:clock
level: 4
---
# tickmonitor

**Uso:** `/ae2 tickmonitor [enable]`

Con `true`, anota cada segundo cuánto tiempo tardó AE2 por tick de nivel; `false` lo detiene. Sin valor, muestra si está activo. Sirve para ver si las redes ME ralentizan el servidor.

<GameScene zoom="4" interactive={true}>
  <Block id="ae2:controller" x="0" y="0" z="0" />
  <Block id="ae2:controller" x="1" y="0" z="0" />
  <Block id="ae2:controller" x="0" y="1" z="0" />
  <Block id="ae2:controller" x="1" y="1" z="0" />
  <IsometricCamera yaw="195" pitch="30" />
</GameScene>
