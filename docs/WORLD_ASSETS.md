# World assets in the Rust client

The client renders the BSP world with the game's own content instead of flat colours.

## Layers

| Layer | Where | Notes |
| --- | --- | --- |
| Virtual file system | `qf-engine::content::vfs` | Mounts every `*.pk3` of a game directory (alphabetical, later packs override earlier ones) plus the map pk3. Shared with the audio loader (`game_sound::mount_content`). |
| Shader scripts | `qf-engine::content::shader` | Parses `scripts/*.shader`. First definition wins. Shaders without a script resolve to an implicit lightmap + diffuse shader. |
| Images | `qf-engine::content::image` | tga/jpg/png via the `image` crate, KTX1 (ETC1/ETC2/BC1-3) via `texture2ddecoder`. The extension is tried in legacy order. Mip chains are generated on the CPU. |
| BSP lightmaps | `qf-engine::content::bsp::lightmap` | Decodes the lightmap lump (128x128 IBSP, 512x512 FBSP), builds one atlas and per-vertex lightmap UVs. Vertex colours are carried through (including patches). |
| GPU side | `qf-client::world`, `bsp.wgsl` | Per-shader batches, one draw per stage, dynamic-offset stage uniforms (tcmod/rgbgen/alphagen are evaluated on the CPU each frame). Movers (submodels) use the same batches. |

`qf-engine` has no GPU or window dependency; all uploads happen in `qf-client`.

## Supported shader subset

`map`, `clampmap`, `animmap`, `material` (diffuse only, lit by the lightmap), `blendfunc`
(including `add`/`filter`/`blend`), `alphafunc`, `rgbgen` (identity, const, vertex,
exactvertex, oneminusvertex, wave basics), `alphagen`, `tcgen` (base, lightmap, environment,
vector), `tcmod` (scroll, scale, rotate, turb, stretch basics), `depthwrite`, `depthfunc`,
`cull`, `sort`, `surfaceparm` (sky, nodraw/skip, trans, nonsolid, nolightmap, fog, water),
`skyparms` (far box).

Missing images are replaced by a checker texture and logged (`texture: '...' missing`).
`nodraw`/`skip` surfaces and shaders are not drawn. Set `QF_SHADER_WARNINGS` to print parser
warnings and `QF_DEBUG_SHADERS` to list the world batches.

## Offscreen screenshot

```
qf-client <map.pk3> --freeplay --screenshot out.png [--shot-eye x,y,z] [--shot-angles p,y,r] [--shot-size WxH]
```

Renders one frame without a window and exits.

## Follow-ups

- Entity and item models (md3/skm) and their skins.
- Dynamic lights, lightgrid lighting for models, lightstyles.
- Post-processing and fog volumes.
- Sky cloud layers (`skyparms` near box), sun, and `deformvertexes`.
- GLSL `material` details (normal maps, gloss), `teamcolor` rgbgen, `blendfunc decal`,
  `polygonoffset`, `endif` conditionals.
- Face culling (surface winding is not yet trusted, everything is double sided).
- Skybox face orientation of the side faces needs a visual check on more maps.
- KTX luminance-alpha (0x8045) images and other uncommon internal formats.
- Lightmap brightness (`LIGHT_SCALE`) and overbright handling versus the legacy renderer.
