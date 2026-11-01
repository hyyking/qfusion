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

## Left out (reported as diagnostics)
- Bezier patch faces (220 in wbomb1) are not tessellated; 180 faces are skipped from the world mesh.
- Flare and foliage faces are skipped (none in wbomb1).
- Patch-face collision is not implemented; trace comparisons must avoid patch-only collision surfaces. This is relevant to the 220 Bezier patch faces in wbomb1.
- Inline submodel collision is not exposed; `Bsp::trace` tests world model 0 only.
- Fog records are size-checked but not decoded.
- Lighting, lightgrid, visibility and lightarray are kept as offset/length only.
- No shader/texture resolution, lightmaps or rendering; parsed entity properties supplement and do not replace the original raw entity string.
- `RBSP` is rejected. `IBSP` v46/47 layouts have synthetic coverage but no real reference map was tested.
- Only model 0 is exported by the CLI; other models are parsed but not meshed by it.
- Overlapping lumps are not rejected; only individual lump bounds are checked.
- Not a content pipeline or renderer; no other maps were tested.
- The reference-map test checks tree traversal against the all-leaf reference scan; a separately captured legacy-engine trace fixture for wbomb1 is not yet available.

## Interface for the future engine
- `Bsp::parse(&[u8])` returns typed lumps, parsed entities, collision structures, and a `diagnostics` list.
- `Bsp::model_mesh(i)` returns `WorldMesh` (positions, normals, uvs, indices, shader index per triangle).
- `Bsp::trace(start, end, mins, maxs, contents_mask)` returns a brush-only point/box trace, independent of GPU or movement code.
- Next steps: patch collision/tessellation, vis and lighting; capture live legacy collision traces for wbomb1.
