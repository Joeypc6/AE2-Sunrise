---
navigation:
  title: "tickmonitor"
  parent: index.md
  position: 5
  icon: minecraft:clock
level: 4
---
# tickmonitor

**Verwendung:** `/ae2 tickmonitor [enable]`

Mit `true` wird jede Sekunde protokolliert, wie lange AE2 pro Level-Tick brauchte; `false` beendet es. Ohne Wert zeigt er, ob es läuft. So sieht man, ob ME-Netzwerke den Server bremsen.

<GameScene zoom="4" interactive={true}>
  <Block id="ae2:controller" x="0" y="0" z="0" />
  <Block id="ae2:controller" x="1" y="0" z="0" />
  <Block id="ae2:controller" x="0" y="1" z="0" />
  <Block id="ae2:controller" x="1" y="1" z="0" />
  <IsometricCamera yaw="195" pitch="30" />
</GameScene>
