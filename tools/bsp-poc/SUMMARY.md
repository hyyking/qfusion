# wbomb1 BSP reader POC - Summary

## Goal
Verify that the existing Warsow/qfusion `wbomb1` map can be read directly, and decode enough world geometry to produce verifiable output. Independent of the window, audio and networking POCs.

## Asset
- Not present in the checkout or submodules.
- Found read-only in an installed Warsow 2.1: `C:\Program Files (x86)\Warsow 2.1\basewsw\map_wbomb1.pk3`, entry `maps/wbomb1.bsp` (19,893,476 bytes).
- Nothing was copied into the repository, modified or downloaded.
- Format: `FBSP` version 1, 18 lumps (layouts from `source/common/facilities/qfiles.h`).

## Commands
```
cd tools/bsp-poc
cargo test
cargo run --release -- "<basewsw>\map_wbomb1.pk3" --entry maps/wbomb1.bsp --obj out.obj
```
The smoke test reads `WARSOW_BASEWSW` (default `C:\Program Files (x86)\Warsow 2.1\basewsw`) and skips with a message if the map is missing.

## What was tested
- Header, magic, version and lump table, with bounds, negative-offset and record-size checks.
- Decoding of entities, shaders, planes, models, vertices, elements and faces; index range validation.
- World mesh from planar and trisurf faces, plus OBJ export.
- Unit tests (synthetic legal FBSP): valid parse, truncation at many points, bad magic/version, RBSP rejection, out-of-bounds/misaligned lumps, bad face indices.
- Real-map smoke test and CLI run on `wbomb1`:
  67,713 vertices, 61,284 indices, 8,776 faces, 172 shaders, 13,904 planes, 8 models;
  world mesh 55,639 triangles, all indices in range, finite positions.

## Left out (reported as diagnostics)
- Bezier patch faces (220 in wbomb1) are not tessellated; 180 faces are skipped from the world mesh.
- Flare and foliage faces are skipped (none in wbomb1).
- Nodes, leafs, brushes, brushsides and fogs are only size-checked: no collision, no BSP tree walk.
- Leaf-face and leaf-brush lumps are not inspected.
- Lighting, lightgrid, visibility and lightarray are kept as offset/length only.
- No shader/texture resolution, lightmaps or rendering; entities are a raw string, not parsed.
- `RBSP` is rejected. `IBSP` v46/47 layouts are coded but untested (no legal test data).
- Only model 0 is exported by the CLI; other models are parsed but not meshed by it.
- Overlapping lumps are not rejected; only individual lump bounds are checked.
- Not a content pipeline or renderer; no other maps were tested.

## Interface for the future engine
- `Bsp::parse(&[u8])` returns typed lumps plus a `diagnostics` list.
- `Bsp::model_mesh(i)` returns `WorldMesh` (positions, normals, uvs, indices, shader index per triangle).
- Next steps: patch tessellation, brush/plane decoding for collision, vis and lighting.
