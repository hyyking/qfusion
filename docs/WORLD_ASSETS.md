# World assets in the Rust client

The client renders the BSP world with the game's own content instead of flat colours.

## Layers

| Layer | Where | Notes |
| --- | --- | --- |
| Virtual file system | `qf-engine::content::vfs` | Mounts every `*.pk3` of a game directory (alphabetical, later packs override earlier ones) plus the map pk3. Shared with the audio loader (`game_sound::mount_content`). |
| Shader scripts | `qf-engine::content::shader` | Parses `scripts/*.shader`. First definition wins. Shaders without a script resolve to an implicit lightmap + diffuse shader. |
| Images | `qf-engine::content::image` | tga/jpg/png via the `image` crate, KTX1 via `texture2ddecoder` (ETC1/ETC2/BC1-3) or direct 8-bit decode (ALPHA8 0x803C, LUMINANCE8, LUMINANCE8_ALPHA8 0x8045, RGB/RGBA/BGR/BGRA). Format is sniffed from the magic, so KTX data stored as `.tga` works. Stored KTX mips are used when the chain is complete, otherwise mips are generated on the CPU (`load_mip_chain`). A built-in TGA decoder handles 24 bpp files with alpha descriptor bits that the `image` crate rejects. `$whiteimage`, `*black` and `*grey` are built in. |
| BSP lightmaps | `qf-engine::content::bsp::lightmap` | Decodes the lightmap lump (128x128 IBSP, 512x512 FBSP), builds one atlas and per-vertex lightmap UVs. Vertex colours are carried through (including patches). |
| GPU side | `qf-client::world`, `bsp.wgsl` | Per-shader batches, one draw per stage, dynamic-offset stage uniforms (tcmod/rgbgen/alphagen are evaluated on the CPU each frame). Movers (submodels) use the same batches. |

`qf-engine` has no GPU or window dependency; all uploads happen in `qf-client`.

## Supported shader subset

`map`, `clampmap`, `animmap`, `material` (diffuse only, lit by the lightmap), `blendfunc`
(including `add`/`filter`/`blend`), `alphafunc`, `rgbgen` (identity, const, vertex,
exactvertex, oneminusvertex, wave basics), `alphagen`, `tcgen` (base, lightmap, environment,
vector), `tcmod` (scroll, scale, rotate, turb, stretch basics), `depthwrite`, `depthfunc`,
`cull`, `sort`, `surfaceparm` (sky, nodraw/skip, trans, nonsolid, nolightmap, fog, water),
`skyparms` (far box), `if`/`endif` conditionals (assumed capabilities: glsl, cube maps, 16384 max texture size; no deluxe maps or portals), `rgbgen`/`alphagen` incl. `lightingidentity`, `distanceramp` (far value), `teamcolor`/`teamcolorwave` (rendered as identity), `colorwave`, `blendfunc decal`/`filterlighting` (as legacy fallback), `material` without arguments (shader name), `animclampmap`.

Directives that are deliberately ignored without a warning are listed in `IGNORED_SHADER_DIRECTIVES`, `IGNORED_SHADER_PREFIXES` (`qer_`, `q3map_`) and `IGNORED_STAGE_DIRECTIVES` in `shader.rs`, so every remaining warning is actionable. All 15 warnings left on the official maps are typos in the content (`identitiy`, `sutfaceparm`, `rbggen`, ...), which legacy also rejects.

## Auditing content

`cargo run --release -p qf-engine --bin world-audit -- <game-dir> [--map x.pk3] [--verbose]` parses shaders and resolves every BSP shader image and builtin/speaker sound headlessly for each map. Official 2.1 data (38 maps): warnings 7790 -> 608 (15 unique), checker (unreadable) textures 60 -> 0, missing textures 64 -> 56 (13 unique: `noshader` and genuinely absent content such as `textures/facility/concrete3`).

## Known missing content

`sounds/world/tele_out` (also requested by legacy `S_TELEPORT_OUT`) is not in any official pack; the resolver already tries `.ogg` and `.wav`, so this is not a lookup bug and the preload report is expected.

Missing images are replaced by a checker texture and logged (`texture: '...' missing`).
`nodraw`/`skip` surfaces and shaders are not drawn. Set `QF_SHADER_WARNINGS` to print parser
warnings and `QF_DEBUG_SHADERS` to list the world batches.

## Offscreen screenshot

```
qf-client <map.pk3> --freeplay --screenshot out.png [--shot-eye x,y,z] [--shot-angles p,y,r] [--shot-size WxH]
```

Renders one frame without a window and exits.

## Entities

`world-audit` also prints the classnames that have no simulated behaviour and the item classnames that `Pickups` does not spawn. On the 38 official maps the client currently spawns only triggers, targets and movers (`MapEntities`); `Pickups` (items, weapons, ammo, powerups) exists in `qf-engine::game` but the client does not instantiate it, and there is no md3/skm model loader, so no item, flag or `misc_*` model is drawn. `item_health` (9) and `ammo_shells` (1) are not legacy classnames either. Other unhandled classnames: `info_player_*` (used by the spawn picker, not simulated), `team_ctf_*`, `trigger_capture_area`, `misc_portal_*`, `misc_particles`, `target_speaker` (audio only), `target_location`, `func_explosive`.

## Follow-ups

- Entity and item models (md3/skm) and their skins.
- Dynamic lights, lightgrid lighting for models, lightstyles.
- Post-processing and fog volumes.
- Sky cloud layers (`skyparms` near box), sun, and `deformvertexes`.
- GLSL `material` details (normal maps, gloss), real team colours and viewer distance for
  `distanceramp`, `videomap`/`surroundmap`.
- Face culling (surface winding is not yet trusted, everything is double sided).
- Skybox face orientation of the side faces needs a visual check on more maps.
- Lightgrid/lightarray lumps are not decoded; they only light models, so they wait for model rendering.
- Lightmap brightness (`LIGHT_SCALE`) and overbright handling versus the legacy renderer.
