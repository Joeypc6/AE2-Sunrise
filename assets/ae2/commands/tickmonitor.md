---
navigation:
  title: "tickmonitor"
  parent: index.md
  position: 5
  icon: minecraft:clock
level: 4
---
# tickmonitor

**Usage:** `/ae2 tickmonitor [enable]`

With `true`, logs once a second how much time AE2 took per level tick; `false` stops it. Without a value it shows whether it is on. Use it to see whether ME networks slow the server.

<GameScene zoom="4" interactive={true}>
  <Block id="ae2:controller" x="0" y="0" z="0" />
  <Block id="ae2:controller" x="1" y="0" z="0" />
  <Block id="ae2:controller" x="0" y="1" z="0" />
  <Block id="ae2:controller" x="1" y="1" z="0" />
  <IsometricCamera yaw="195" pitch="30" />
</GameScene>
