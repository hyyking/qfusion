# qfusion engine crate - Summary

## Goal
The GPU- and window-independent engine crate reads Warsow/qfusion BSP/PK3 content, parses entities, exposes collision traces, contains fixed-step noclip player simulation, and a port of the legacy Warsow `Pmove` (`simulation::pmove`) with a walking player (`simulation::walk`) over BSP brush collision (`simulation::bsp_world`).

## Asset
- Not present in the checkout or submodules.
- Found read-only in an installed Warsow 2.1: `C:\Program Files (x86)\Warsow 2.1\basewsw\map_wbomb1.pk3`, entry `maps/wbomb1.bsp` (19,893,476 bytes).
- Nothing was copied into the repository, modified or downloaded.
- Format: `FBSP` version 1, 18 lumps (layouts from `source/common/facilities/qfiles.h`).

## Commands
```
cargo test -p qf-engine
cargo run -p qf-engine --release --bin bsp-inspect -- "<basewsw>\map_wbomb1.pk3" --entry maps/wbomb1.bsp --obj out.obj
```
The smoke test reads `WARSOW_BASEWSW` (default `C:\Program Files (x86)\Warsow 2.1\basewsw`) and skips with a message if the map is missing.

## What was tested
- Header, magic, version and lump table, with bounds, negative-offset and record-size checks.
- Decoding of entities, shaders, planes, models, vertices, elements and faces; index range validation.
- Decoding and cross-validation of nodes, leafs, leaf-face/leaf-brush references, brushes and brushsides for FBSP and IBSP.
- Quake entity key/value parsing with quoted-string escapes and line/block comments.
- Point and swept axis-aligned box traces against BSP brushes, including contents masks, start-solid/all-solid state, hit plane and shader surface flags.
- World mesh from planar and trisurf faces, plus OBJ export.
- Unit tests (synthetic FBSP/IBSP): valid parse, truncation, bad magic/version, RBSP rejection, malformed entity strings, out-of-bounds/misaligned lumps, bad geometry references, cyclic nodes, and point/box trace golden values.
- No third-party collision crate was selected: the trace contract is Qfusion-specific, so the brush clip and BSP traversal equations were ported directly from the legacy implementation.
- Trace goldens use a six-plane cube and legacy `Ops::ClipBoxToBrush` / `RecursiveHullCheck` equations (`DIST_EPSILON = 1/32`); comparisons allow `1e-5` fraction and `0.05` engine units for positions, independently of movement.
- On the installed wbomb1 map, BSP-tree traces are compared against a reference scan of all leaf brushes using the same legacy brush-clipping equations.
- Real-map smoke test on `wbomb1`:
  67,713 vertices, 61,284 indices, 8,776 faces, 172 shaders, 13,904 planes, 8 models;
  237 entities, 3,244 nodes, 3,253 leafs, 3,248 brushes, 25,066 brushsides;
  world mesh 55,639 triangles, all indices in range, finite positions.

## Game host
`game::GameHost` embeds Rhai and runs a gametype script at the fixed 125 Hz tick. The script owns the rules and discovers its map objects through `g.entities()`, `g.trace()` and friends; the engine only supplies players (`game::combat`, armor rule), world queries, a seeded RNG and an event queue. Hooks: required `init`, `tick`; optional `start_match`, `on_player_joined`, `on_team_change`, `on_command`, `on_damage`, `hud`. Tested in `tests/game_host.rs` with an inline script.

## Left out (reported as diagnostics)
- Bezier patch faces are tessellated into the world mesh (`bsp::patch`, legacy `r_subdivisions` 5, LOD groups by face bounds).
- Flare and foliage faces are skipped (none in wbomb1).
- Patch collision builds per-quad/triangle facets (legacy `CM_CreatePatch`, flatness 16) and is traced with the brush clipper. Deviations: flatness exponent capped at 6, patches need odd control grids >= 3x3, facet planes capped at 32 with no overflow, submodel patches are not traced, and bounds pre-tests use a swept AABB instead of the SIMD line-distance test.
- `Bsp::trace` tests world model 0 only; inline submodels are only exposed as `Bsp::box_touches_model` (zero-length box contact for trigger volumes, unrotated, no patches).
- Fog records are size-checked but not decoded.
- Lighting, lightgrid, visibility and lightarray are kept as offset/length only.
- No shader/texture resolution, lightmaps or rendering; parsed entity properties supplement and do not replace the original raw entity string.
- `RBSP` is rejected. `IBSP` v46/47 layouts have synthetic coverage but no real reference map was tested.
- Only model 0 is exported by the CLI; other models are parsed but not meshed by it.
- Overlapping lumps are not rejected; only individual lump bounds are checked.
- Not a content pipeline or renderer; no other maps were tested.
- The reference-map test checks tree traversal against the all-leaf reference scan; a separately captured legacy-engine trace fixture for wbomb1 is not yet available.

## Engine interfaces
- `Bsp::parse(&[u8])` returns typed lumps, parsed entities, collision structures, and a `diagnostics` list.
- `Bsp::model_mesh(i)` returns `WorldMesh` (positions, normals, uvs, indices, shader index per triangle).
- `Bsp::trace(start, end, mins, maxs, contents_mask)` returns a brush-only point/box trace, independent of GPU or movement code.
- Player simulation is exposed through `simulation::player`; it uses parsed BSP entities for spawn selection without depending on window or GPU types.
- Rendering, window/input, and audio remain owned by `qf-client`.

## Movement
- `simulation::pmove` ports `gs_pmove.cpp` (ground/air accel, friction, strafe/bunny, jump, dash, wall jump, crouch slide, water, ladders, step/slide collision, landing); fixed 125 Hz; the touch hook (`pmove_with_touch`/`PmoveTouch`) runs triggers after the move and refreshes delta angles, as `gs_pmove.cpp` does.
- `tests/pmove_traces.rs` replays `tests/fixtures/pmove_125hz.csv` (legacy harness recording): 9/9 scenarios pass, exact invariants equal, trajectories within 0.05 (max 0.0326).
- `tests/pmove_bsp.rs` walks on BSP brush collision, kept separate so collision issues are not blamed on movement.
- Gaps: only the 125 Hz recording, no slope/step/crouch-slide fixtures, submodel patch collision missing.

## Map entities (`simulation::entities`)
- Every lump entity is an entity of a private `bevy_ecs` world inside `MapEntities` (components/resources/systems in `entities/{components,spawn,logic,parse}.rs`); no Bevy type is public. `MapEntities::spawn` / `spawn_for_gametype(&[Entity], &impl BrushModels, &Gametype)` instantiate it; `ModeSimulation::set_entities` + `advance_frame_with_entities` run it in the fixed 125 Hz walking tick (level time = tick * 8 ms), `think` runs on non-walking ticks. `drain_trigger_events` reports `JumpPad`, `Teleport`, `Hurt`, `Used`, `Message`, `Kill`, `Telefrag`.
- Spawn filtering follows `G_CanSpawnEntity`: `notfree`, `notduel`, `notteam`, `gametype`, `not_gametype` (plain `spawn` = free for all; `Gametype::bomb()` is team based). `info_null`/`info_camp` are freed at spawn.
- Supported touch entities: `trigger_push` (jump pad via `target`, or angle/speed push, push-once, wait), `trigger_teleport` (+SPECTATOR flag, wait; horizontal speed kept, angles/delta angles, PMF_TIME_TELEPORT, then KillBox), `trigger_hurt` (dmg, wait, START_OFF/TOGGLE/NO_PROTECTION/SLOW/KILL/FALL), `trigger_multiple`/`trigger_once` (wait, NOT_PLAYER, TRIGGERED), `trigger_counter` (count, NOMESSAGE), `trigger_always`, `trigger_relay`.
- Supported actions fired through `target`/`killtarget`/`delay`/`message`: `target_relay`, `target_delay`, `target_kill`, `target_print`, `target_teleporter` (+telefrag), `func_killbox`. Passive: `target_position`, `info_notnull`, `target_push`, `misc_teleporter_dest`, `info_teleport_destination` (+16 z), `trigger_gravity` (legacy only sets `edict_t::gravity`; pmove never reads it).
- Telefrag: `KillBox` hits the occupants given by `set_occupants` (strict box overlap) and reports `Telefrag`; `func_killbox` also reports the local player.
- Official-map survey (38 maps, `all_official_maps_spawn_without_warnings`): every map spawns with no warnings. Still unsupported by count: `target_location` 1149 (HUD-only), spawns (`info_player_*`/`team_ctf_*spawn`, handled by `simulation::player` for dm/start only), items/weapons/ammo, `func_door` 56 (9 maps), `target_explosion` 51 (2 maps, radial knockback), `misc_capture_area_indicator` 45, `trigger_capture_area` 21, `func_rotating` 10, `func_button`/`func_plat` 7 each, `func_door_rotating` 4, `func_explosive` 4, `target_speaker` 6, `misc_portal_*`/skyportals, race timers (`target_starttimer/stoptimer/checkpoint`), `target_give`.
- Deviations: first-match target instead of `G_PickTarget` random pick; `target_delay` ignores `random`; `trigger_relay` RANDOM flag is not ported (the legacy implementation recurses); jump pads whose target is not above them are removed; thinks run at the start of the tick's touch pass; `target_kill`/`target_teleporter` act only while the player is present; no health/damage application; no world-solid early-out in `KillBox`; rotated submodels and patch collision unsupported.
- Tests: unit tests in `entities/tests.rs` (formula, chains, counter, delay, always, killtarget, telefrag, gametype filter, ledger/classname consistency, determinism), `tests/map_entities.rs` (frame-grouping independence, teleport via `ModeSimulation`, wbomb1 and all-official-maps spawn).
