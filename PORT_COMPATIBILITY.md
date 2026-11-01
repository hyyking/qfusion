# Port compatibility decisions

This document records the Stage 1 decisions that gate Stage 2 of
[`ENGINE_PORT.md`](ENGINE_PORT.md). “Compatible” means preserving observable
gameplay behavior, not legacy implementation details or wire formats.

## Targets and runtime constraints

- Client: Windows, Linux, and macOS releases still in mainstream support.
- Dedicated server: Linux headless first. Listen servers are deferred.
- Graphics: `wgpu` on modern Vulkan 1.2, DirectX 12, or Metal-capable hardware;
  the initial performance target remains 144 FPS typical combat and 120 FPS
  1% lows at 1080p/high on an RTX 2080-class GPU.
- Simulation: fixed 125 Hz (8 ms); independent of rendering and network snapshot
  rates. The movement harness, rather than a tick-rate assumption, is the
  compatibility authority.
- Multiplayer: new protocol, Bomb and Defuse only initially, direct IP/hostname
  connection, and server-authoritative gameplay.

## Compatibility matrix

| Area | Status | Decision and boundary |
|---|---|---|
| BSP world surfaces | **converted** | Import the official `wbomb1` FBSP v1 map into engine-owned runtime data. The BSP POC read its header/lumps and built a planar/trisurf mesh; it did not decode collision or tessellate 220 patch faces. Stage 2 must implement and validate the collision/entity data and any visible patch geometry needed by this map. Other BSP families and maps are deferred until individually tested. |
| BSP collision | **compatible** | Preserve collision outcomes for the selected map and movement reference cases. Do not treat the POC's size checks of nodes, leaves, brushes, and brushsides as collision support. |
| Entity parsing and map behavior | **converted** | Parse entities required by `wbomb1` and its Bomb/Defuse rules into engine-owned entities. Port observed map hooks/targets; arbitrary legacy entity classes and map-script APIs are not promised. |
| Movement | **compatible** | Match the existing shared movement behavior at 125 Hz. Event/state transitions are exact; trajectory comparisons use tolerances recorded by the movement reference harness. Cross-platform determinism remains a tested requirement, not an assumed property. |
| Weapons and damage | **compatible** | Preserve both fire modes, timing, damage, knockback, ammo, and projectile behavior for the initial mode's arsenal: Gunblade, Machinegun, Riotgun, Grenade Launcher, Rocket Launcher, Plasmagun, Lasergun, Electrobolt, Shockwave, and Instagun. |
| Pickups | **compatible** | Preserve health, armor, weapon/ammo, and powerup effects used by the selected mode/map. Unused legacy item classes and mode-specific exceptions are deferred until required by another supported map or mode. |
| Game modes and match rules | **compatible** | Bomb and Defuse rules only, including objective actions, round/win conditions, and rule-visible scoring. Other modes are deferred. |
| Match lifecycle, teams, spawns, scoring | **compatible** | Preserve observable round/match transitions, team assignment, spawn selection, respawn, and score results for Bomb and Defuse. Record deterministic cases and compare state, not presentation timing. |
| Server commands | **converted** | Provide new commands for the supported administrative/gameplay actions (start, status, score, and map change). Legacy command names, aliases, and unrelated server tooling are not compatibility requirements. |
| Maps and assets | **converted** | Import `maps/wbomb1.bsp` and the official `basewsw` assets it references. The exact dependency manifest must be generated from the installed reference content and checked into the importer tests. Other official maps are deferred; community content is dropped from the initial scope. |
| Scripts | **converted** | Re-express built-in Bomb/Defuse behavior from `source/server/game/progs/gametypes/bomb.gtd` and `bomb/*.as`, plus the behavior encoded in the `wbomb1` map entities, in the new internal scripting/game-rule layer. AngelScript source/API compatibility and user-authored scripts are dropped for the initial release; additional built-in modes are deferred. |
| Configuration and map rotation | **converted** | Carry forward only settings needed to reproduce the supported mode, map, and server behavior. Initial rotation contains `wbomb1`; other rotation entries and arbitrary legacy config syntax are deferred. |
| Network protocol | **dropped** | No interoperability with existing qfusion/Warsow clients or servers. The new client and server must be deployed together. |
| Legacy demos | **dropped** | Existing demo playback is not an initial-release requirement. A demo format for the new protocol is deferred and must be decided separately. |

### Content roots and precedence

The legacy source was inspected before the importer decision (`files.cpp`,
`win_fs.cpp`, and `unix_fs.cpp` under `source/common/`; the executable-adjacent
layout is also documented in `README.md`):

- The executable-adjacent root is `fs_basepath/basewsw` (`fs_basepath` defaults
  to `.`); the README also instructs running with a sibling `basewsw` directory.
- When `fs_usehomedir` is enabled (default on public builds and enabled by the
  Unix launcher), the legacy per-user root is added ahead of the executable
  data root, so a per-user copy takes precedence on conflicts.
- On Windows, `USE_MY_DOCUMENTS` is defined: the source calls
  `CSIDL_PERSONAL` and appends `My Games/Warsow <major>.<minor>`. This is
  **Documents/My Games, not `%APPDATA%`**.
- On Linux, the source uses `$XDG_DATA_HOME` when set, otherwise
  `$HOME/.local/share`; on macOS it uses `$HOME/Library/Application Support`.
  The application/version directory is appended in each case.

Therefore, the existing source verifies executable-adjacent discovery and a
user-root override, but does not verify a Windows AppData `basewsw` tree. The
new importer will resolve the per-user root with
`directories::ProjectDirs::from("net", "warsow", "Warsow").data_dir()`, then
append `basewsw`. This gives the Windows per-user AppData root and the
platform-appropriate Linux/macOS data root. Search in descending precedence:
that root, the legacy Windows Documents/My Games root (import-only fallback),
then the executable-adjacent root. A user copy wins over an adjacent copy, and
every imported source records its actual root and content hash. The importer
must not write into any source root.

Before importer implementation, run a root-discovery fixture with the same
relative test asset in both roots and assert that the user copy is selected and
reported; repeat with only one copy in each root. The current checkout does
not include `basewsw` assets, so real-install discovery remains an environment
check rather than a claim that this checkout contains the data. Treat the
fixture as a Stage 1 gate: do not implement the importer until it passes for
executable-adjacent and per-user AppData roots.

### Offline slice map

`wbomb1` is the chosen offline slice map, as well as the first planned
multiplayer map. [PR #1](https://github.com/hyyking/qfusion/pull/1) reports a
real Warsow 2.1 `map_wbomb1.pk3` containing
`maps/wbomb1.bsp` (FBSP v1), with a working bounded POC parser and world mesh.
It is a suitable integration target, but not yet a complete slice map: collision
brushes and patch tessellation remain unimplemented in that POC. Stage 2 must
prove collision traversal and movement cases on the actual map before calling
the slice complete. Do not silently substitute another map if that gate fails;
record a revised map decision first.

## Content and runtime dependencies

- Use Rust crate `zip` 2.x, with default features disabled and `deflate`
  enabled, to read `.pk3` source archives (the feature set used by PR #1).
- Keep a bounded, engine-owned BSP reader for FBSP v1 and the selected map's
  collision/entity structures. Do not add a generic BSP crate or `binrw` for
  this path; the reader must validate all offsets, lengths, and indices.
- Load the selected map and its archive assets through `zip` plus the same
  bounded BSP reader for the offline slice; convert source data into
  documented, versioned engine-owned runtime structures. No generic runtime
  asset/BSP loader is selected. `wgpu` owns GPU resources and must not leak into
  parsing or simulation interfaces.
- Use `directories` for per-user data-root resolution. Keep the existing
  direction of `wgpu`, `winit`, `bevy_ecs` behind engine-owned interfaces,
  and Rhai for internal built-in scripts. Exact crate versions beyond the POC's
  `zip` 2.x are pinned in the implementation lockfile when the Rust workspace
  is created; they are not claims of compatibility made by this document.

## Reference scenarios

Run each case against a fixed reference build, `wbomb1` content hash, mode
configuration, 125 Hz tick, and recorded input/command sequence. Save the
reference output with the run; replay it unchanged for each port comparison.
Compare discrete outcomes exactly (connect/result, entity/event identity,
health/ammo/score, team, match state, and map name); compare movement
trajectories using the harness tolerances. The old and new network protocols
are not compared packet-for-packet.

| ID | Repeatable procedure | Record and compare |
|---|---|---|
| R1 Connect | Start a local reference server with the fixed test config; connect one client by loopback IP, then repeat with a rejected password. | Accepted/rejected result, client slot, team/state, and server log outcome. |
| R2 Load map | Issue the map-load action for `wbomb1` from a clean server start. | Loaded map identity, parsed entity counts/IDs, spawn/objective entities, and load diagnostics. |
| R3 Spawn | Spawn each supported team/player at the fixed initial state; repeat after death/round reset. | Spawn point/team, origin, health, armor, weapon, and respawn/round events. |
| R4 Move | Replay the movement harness inputs on `wbomb1`, including hold/tap jump, air direction change, wall hit during special moves, and fast/slow landings. | Per-tick origin, velocity, grounded state, and movement events; run twice and compare exact fields and numeric tolerances. |
| R5 Weapons | At fixed range and target state, exercise weak and strong fire for Gunblade, Machinegun, Riotgun, Grenade Launcher, Rocket Launcher, Plasmagun, Lasergun, Electrobolt, Shockwave, and Instagun; include an empty-ammo case. | Fire/projectile/hit events, damage, self-damage, knockback, ammo, and refire timing. |
| R6 Pickups | Touch representative health, armor, ammo/weapon, and powerup pickups at empty, partial, and capped inventory. | Pickup event, inventory delta/cap, and score/health/armor result. |
| R7 Score | Complete a deterministic kill/objective scoring sequence for each Bomb/Defuse outcome. | Individual/team score and exact round result. |
| R8 Match state | Replay ready-up, waiting, live round, round end, and match end with fixed player counts. | State transitions, timers, team assignment, and winner. |
| R9 Map change | Change from `wbomb1` to itself after a round, then to a second official map when that map enters the supported corpus. | Server map transition, reset state, and resulting map/entity identity. |

R1 and R9 can compare user-visible server behavior but not legacy wire messages.
R4 is blocked on the movement harness in
[issue #4](https://github.com/hyyking/qfusion/issues/4); until its trace fixture is
available, this scenario list is defined but the Stage 1 exit condition is not
met. The harness must be run twice to establish repeatability before its trace
is used as a port acceptance baseline.

## Intentional-difference log

Every deviation from this matrix must be recorded here before it is accepted.
Add one row per difference with: ID, area, legacy reference/scenario, intended
new behavior, rationale, compatibility impact, validation evidence, approver,
and status (`proposed`, `accepted`, or `rejected`). An accepted difference
changes the relevant matrix decision and is reviewed with the same change; do
not waive a mismatch solely because it is difficult to reproduce.

| ID | Area | Difference / rationale | Scenario and evidence | Approver | Status |
|---|---|---|---|---|---|
| — | — | No intentional behavioral differences approved yet. | — | — | — |

## Risks and ownership

| Risk from `ENGINE_PORT.md` | Owner / disposition |
|---|---|
| Content: implicit BSP collision/entity semantics and incomplete map assets | @hyyking owns the `wbomb1` import manifest and collision gate. Defer broader map/content support until that map passes R2, R3, R4, and R6. |
| Scripts: AngelScript hooks may encode uncaptured behavior | @hyyking owns the built-in Bomb/Defuse and `wbomb1` behavior inventory. Defer compatibility with arbitrary AngelScript and user scripts; compare supported hooks through R2 and R7–R9. |
| Determinism: float/collision differences may diverge across OSes | @hyyking owns the movement harness acceptance gate. Defer cross-platform sign-off until R4 repeats on Windows, Linux, and macOS with documented tolerances. |
| Protocol: new protocol is not legacy interoperable | Accepted product boundary; multiplayer interoperability tests are deferred to the new client/server rollout. |
| Crate/version and importer risks | Dependency names and responsibilities are decided above. Pin and audit exact versions when the Rust workspace is created; Stage 2 must not start with the collision dependency treated as pre-approved. |

Stage 2 remains gated on a runnable movement trace harness, recorded reference
outputs for the scenarios above, and a successful `wbomb1` collision test. The
source-level root behavior is documented here; root-fixture and real-install
checks remain explicit implementation gates.
