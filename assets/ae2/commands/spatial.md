---
navigation:
  title: "spatial"
  parent: index.md
  position: 7
  icon: ae2:spatial_io_port
level: 4
---
# spatial

**Usage:** `/ae2 spatial [info <plot> | tp <plot> | tpback | givecell <plot>]`

Works with spatial storage plots, the spaces a spatial cell holds:

- without more, lists the plots;
- `info <plot>`: its size, owner and where it was taken from;
- `tp <plot>`: takes you into the plot;
- `tpback`: takes you back to where you were;
- `givecell <plot>`: gives you a spatial cell holding that plot.

<GameScene zoom="3" interactive={true}>
  <Block id="ae2:spatial_io_port" x="0" y="0" z="0" />
  <Block id="ae2:spatial_pylon" x="1" y="0" z="0" />
  <Block id="ae2:spatial_pylon" x="2" y="0" z="0" />
  <Block id="ae2:spatial_pylon" x="3" y="0" z="0" />
  <Block id="ae2:spatial_pylon" x="0" y="0" z="1" />
  <Block id="ae2:spatial_pylon" x="0" y="0" z="2" />
  <IsometricCamera yaw="210" pitch="30" />
</GameScene>

<Row>
  <ItemImage id="ae2:spatial_storage_cell_2" scale="2" />
  <ItemImage id="ae2:spatial_storage_cell_16" scale="2" />
  <ItemImage id="ae2:spatial_storage_cell_128" scale="2" />
</Row>
