---
navigation:
  title: "spatial"
  parent: index.md
  position: 7
  icon: ae2:spatial_io_port
level: 4
---
# spatial

**Uso:** `/ae2 spatial [info <plot> | tp <plot> | tpback | givecell <plot>]`

Trabaja con las parcelas de almacenamiento espacial, los espacios que guarda una celda espacial:

- sin más, lista las parcelas;
- `info <plot>`: su tamaño, dueño y de dónde se tomó;
- `tp <plot>`: te lleva dentro de la parcela;
- `tpback`: te devuelve adonde estabas;
- `givecell <plot>`: te da una celda espacial con esa parcela.

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
