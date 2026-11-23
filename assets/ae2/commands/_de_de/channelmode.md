---
navigation:
  title: "channelmode"
  parent: index.md
  position: 6
  icon: ae2:fluix_smart_cable
level: 4
---
# channelmode

**Verwendung:** `/ae2 channelmode [infinite|default|x2|x3|x4]`

Ohne Modus zeigt er den Kanalmodus. Mit einem setzt er ihn für alle Netzwerke und speichert ihn in der Konfiguration:

- `default`: 8 Kanäle pro Kabel, 32 pro dichtem Kabel;
- `x2`, `x3`, `x4`: das Zwei-, Drei- oder Vierfache;
- `infinite`: keine Kanalgrenzen.

Alle Netzwerke berechnen ihre Kanäle sofort neu.

<GameScene zoom="4" interactive={true}>
  <Block id="ae2:controller" x="0" y="0" z="0" />
  <Block id="ae2:drive" x="1" y="0" z="0" />
  <Block id="ae2:drive" x="-1" y="0" z="0" />
  <IsometricCamera yaw="195" pitch="30" />
</GameScene>
