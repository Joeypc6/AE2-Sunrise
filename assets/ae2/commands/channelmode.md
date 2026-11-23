---
navigation:
  title: "channelmode"
  parent: index.md
  position: 6
  icon: ae2:fluix_smart_cable
level: 4
---
# channelmode

**Usage:** `/ae2 channelmode [infinite|default|x2|x3|x4]`

Without a mode, shows the channel mode. With one, sets it for every network and saves it to the configuration:

- `default`: 8 channels a cable, 32 a dense cable;
- `x2`, `x3`, `x4`: two, three or four times as many;
- `infinite`: no channel limits.

Every network works its channels out again at once.

<GameScene zoom="4" interactive={true}>
  <Block id="ae2:controller" x="0" y="0" z="0" />
  <Block id="ae2:drive" x="1" y="0" z="0" />
  <Block id="ae2:drive" x="-1" y="0" z="0" />
  <IsometricCamera yaw="195" pitch="30" />
</GameScene>
