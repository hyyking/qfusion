# Engine port

## Purpose

This document outlines a full engine port: a new implementation of the qfusion/Warsow engine, including its runtime, platform layer, gameplay simulation, networking, content pipeline, and client presentation.

The existing engine should be treated as the behavioral reference. Port gameplay and content intentionally; do not assume that translating source files one-for-one will produce a maintainable or compatible engine.

## Stack direction

The following choices have been made for the initial port direction:

| Concern | Direction |
|---|---|
| Target platforms | Desktop: Windows, Linux, and macOS |
| OS support policy | Current releases still in mainstream support |
| Implementation language | Rust |
| Graphics | `wgpu` |
| Windowing and input | `winit` with separate Rust crates for other platform services |
| Audio | `cpal` for device/output access, with a game-audio layer built for mixing and spatial sound |
| Scripting | Rhai for embedded, Rust-like gameplay and map scripts |
| UI | Custom in-game HUD and menus; `egui` for developer/debug tools |
| Entity model | `bevy_ecs` behind engine-owned game-facing interfaces |
| Performance target | At 1080p/high/no ray tracing on an RTX 2080-class GPU: typical combat holds 144 FPS, with 1% lows near 120 FPS; modern Vulkan 1.2 / DirectX 12 / Metal-capable GPUs |
| Network interoperability | A new protocol; compatibility with existing qfusion/Warsow clients and servers is not required |
| Multiplayer rollout | Bomb and Defuse as the only initial multiplayer mode; dedicated servers first, listen servers later |
| Initial server discovery and identity | Direct IP/connect; accountless names and optional server passwords; no browser, matchmaking, or account backend initially |
| Dedicated server platform | Linux headless first |
| Initial anti-cheat scope | Server-authoritative validation and movement limits; no external anti-cheat initially |
| Initial multiplayer map | `wbomb1`, then expand map support after gameplay is stable |
| Initial network update target | 125 Hz snapshots, independent from the 125 Hz simulation tick |
| Hitscan lag compensation | Adaptive server rewind, capped at 75 ms |
| Runtime content | Import/conversion tools produce documented runtime formats; initial converters target chosen official content only |
| Reference content roots | `basewsw` beside the executable and `basewsw` in the user's application-data directory; AppData copy overrides on conflicts |
| User modding | Rhai scripts are internal to built-in maps and modes initially; user-authored mods are deferred |
| Visual direction | Keep open until a rendered map prototype is available |

The port will own its engine architecture rather than adopt the full Bevy game framework. Use `wgpu` directly for rendering so the engine controls its render architecture and BSP-specific pipeline. Bevy ecosystem crates remain candidates for focused infrastructure (for example, ECS or application scheduling) when they fit behind explicit interfaces; avoid making core simulation or content formats depend on Bevy-specific APIs without a demonstrated benefit.

This is not a presumption that Bevy is inherently too slow. Bevy is built on `wgpu` and can be performant; the deciding factor here is architectural ownership and keeping the option to tailor the runtime and renderer to the game. Direct `wgpu` also means more engine infrastructure must be built and maintained. Compare real profiling results before adding complexity solely for a hypothetical performance gain.

These are direction-setting choices, not a final dependency manifest. Specific crate versions and the content pipeline remain open. Keep those decisions explicit until a vertical slice demonstrates that the selected components work together on all target platforms.

## Physics and movement fidelity

Movement physics are a defining part of Warsow's gameplay and must be treated as a core compatibility requirement, alongside map loading. The port should reproduce the existing movement feel deliberately—not substitute generic character-controller behavior or tune only by eye. The current shared movement implementation in `source/common/facilities/gs_pmove.cpp` is the primary reference; collision and player dimensions are also defined in `source/common/facilities/`.

Before implementation, capture and document the existing movement rules and their interactions with collision: acceleration and friction on ground, air acceleration and strafe-bunny acceleration, jump and dash-jump timing, wall jumps, crouch sliding, gravity, landing, slopes/steps, water and ladders, and collision response. The list should be verified against code and in-game behavior rather than assumed complete.

Build a repeatable reference harness that feeds recorded input-command sequences and map/collision setups to the existing engine, recording position, velocity, grounded state, and movement events at each simulation step. Use those traces as regression targets for the Rust implementation. Include edge cases such as holding or tapping jump, changing direction in mid-air, hitting walls during special moves, landing at different speeds, and movement at supported simulation rates. Separate exact invariants (such as event/state transitions) from floating-point tolerances for trajectories.

Target a fixed **125 Hz simulation tick** (8 ms per tick) initially. Keep this independent from rendering and the network snapshot rate; the initial snapshot target is also 125 Hz, subject to separate measurement and configuration. Use the same shared movement implementation for local play, later server authority, and client prediction wherever the architecture permits. Define input sampling explicitly and verify that results do not depend on render frame rate.

The desired standard is deterministic simulation and prediction for identical inputs across Windows, Linux, and macOS. Test this with repeatable traces on each platform and document any remaining numeric tolerance. If strict cross-platform determinism conflicts with matching current movement behavior, surface the conflict before changing physics semantics or reducing the determinism requirement.

The current direction is to evaluate Rust collision crates and adapt movement to their collision API rather than immediately porting all collision code. This is a hypothesis to validate, not permission to accept changed collision behavior: use the movement/collision reference harness to check candidate collision implementations and reject or wrap one that cannot preserve required gameplay.

**Acceptance requirement:** the offline slice is not considered movement-complete until its recorded movement cases match the reference within documented tolerances, including the special movement techniques that define Warsow. Map collision fidelity is part of this requirement: a movement mismatch caused by incorrect BSP collision must be distinguished from one caused by movement integration.

## Initial multiplayer release

The first multiplayer release targets **Bomb and Defuse only**, preserving the existing mode's rules and objective behavior as closely as practical. It uses a dedicated, authoritative headless server for Linux; listen-server hosting is deferred. Players connect directly by IP or hostname; accountless player names and optional server passwords avoid making an account or matchmaking backend a prerequisite. An in-game browser, master server, parties, and account identity are out of scope initially.

The protocol is new and uses custom UDP transport. The server validates gameplay state and movement limits; external anti-cheat is deferred. Clients predict their own movement and reconcile to server authority, while interpolating remote players. For hitscan weapons, use adaptive server-side lag compensation with a hard maximum rewind window of **75 ms**. Target **125 snapshots per second** to align with the 125 Hz simulation, while keeping the two rates independently configurable so testing can measure the cost and behavior separately. The initial multiplayer target should remain playable under **200 ms RTT and 3% packet loss**.

Use high-frequency datagrams for time-sensitive input and state, not as a reliable stream. Candidate techniques include sequenced, unreliable snapshots; delta-compressing against a snapshot the client has acknowledged; periodic full snapshots to recover a baseline; quantized state with explicit precision bounds; and separate reliable delivery for infrequent control/match events. Keep datagrams below a conservative path-MTU budget to avoid IP fragmentation. The server should pace packets, track bandwidth and loss per client, and expose snapshot size, send rate, and client correction metrics. Evaluate bounded input redundancy and snapshot-baseline recovery under the planned packet-loss tests. Do not retransmit obsolete movement snapshots: newer state supersedes them.

Start Bomb and Defuse map support with the official `wbomb1`; add further official maps after gameplay is stable. Initial converters target chosen official maps/assets only; community content import is deferred. Verify that the expected `basewsw` data is discoverable before implementing the importer. The offline movement/rendering slice can use this BSP if it provides suitable test spaces, otherwise use another representative map without expanding the initial multiplayer map commitment.

## Initial vertical slice

The first playable milestone is an **offline single-player test map**. It should load an existing qfusion/Warsow BSP directly and support player movement, one weapon, and pickups. This deliberately validates map parsing and core gameplay without making network transport a dependency of the first prototype.

The slice should establish the Rust application/runtime, `winit` input and window lifecycle, `wgpu` rendering, BSP reading and collision, and enough game state to move, fire, and collect items. Movement and BSP collision should be implemented and compared against recorded reference cases early; do not defer movement fidelity until after rendering polish. Use `cpal` plus a minimal game-audio layer only if basic sound is needed to validate the slice; full mixing and spatial audio can follow. Keep gameplay interfaces compatible with later server authority, but do not build a parallel server/client protocol into this milestone.

Rhai is the selected embedded scripting direction for gameplay and map scripts. For the first slice, scripting should be introduced only where the chosen map or gameplay behavior requires it; core movement and collision should remain testable without running scripts. Use `egui` for developer/debug tooling, not as the production HUD or menu renderer.

**Exit condition:** launch the desktop prototype, load a real existing BSP, navigate its collision, fire the initial weapon, and collect a pickup. Record unsupported BSP features and content dependencies rather than silently substituting a new map format.

## Goals and compatibility

The initial goal is to preserve the recognizable game experience and reuse selected existing game content where practical. Treat the existing `basewsw` directories beside the executable and in the user's application-data directory as read-only reference sources for import. On Windows, the user-data root is the appropriate per-user AppData location; use the corresponding per-user application-data location on Linux and macOS. If an asset exists in both roots, the per-user copy overrides the executable-adjacent copy. Importers should report which root supplied each source asset and write converted output to the new engine's own content/cache location rather than modifying either reference tree.

Import/conversion tools should produce documented runtime formats; full direct runtime support for every legacy asset format is not required. Preserve Warsow's existing visual style unless the map prototype motivates an intentional redesign. In particular, investigate and document:

- BSP map loading, collision, entity parsing, and map-specific behavior.
- Player movement and physics, weapons, damage, pickups, and game-mode rules.
- Match lifecycle, teams, spawn behavior, scoring, and server commands.
- Existing assets, scripts, configuration, and map rotation.
- The client/server protocol and demo formats, if interoperability is required.

Compatibility should be decided explicitly for each area. The network protocol is intended to be new, so old clients and servers will not interoperate. The first multiplayer release targets Bomb and Defuse only. Support selected official maps and assets through conversion; community content import, broad legacy content coverage, and demo compatibility are not assumed. Rhai is an internal scripting mechanism initially, not a user-facing modding promise.

Non-goals for the first milestone:

- Porting every game mode, bot behavior, tool, or platform before a playable slice exists.
- Preserving implementation details that are not observable behavior or required compatibility.
- Replacing all content formats before proving that existing content can be loaded.

## Existing implementation as reference

The current responsibilities are distributed across:

| Area | Current location |
| --- | --- |
| Shared movement, collision-facing gameplay data, and item definitions | `source/common/facilities/` |
| Server runtime, networking, map transitions, and client slots | `source/server/` |
| Game simulation, entities, scoring, and match flow | `source/server/game/` |
| Gametype and map scripts | `source/server/game/progs/gametypes/` and `source/server/game/progs/maps/` |
| Client rendering, input, sound, and UI | `source/client/` |
| Build and dependency guidance | `source/CMakeLists.txt` and `README.md` |

Use these as investigation starting points, not as an architectural mandate for the port. Record observable behavior and edge cases before replacing a subsystem.

## Target architecture

Keep the new engine divided into components with explicit ownership and narrow interfaces. Use Rust modules and crates where they clarify ownership and build boundaries; avoid splitting the first vertical slice into many independently versioned packages. The exact module boundaries may evolve, but a useful initial separation is:

1. **Platform and runtime:** windowing, input devices, clocks, files, threads, and platform-specific startup.
2. **Content:** virtual filesystem, asset access, BSP parsing, map entities, and resource lifecycle.
3. **World and simulation:** collision, authoritative state, reference-matched movement physics, weapons, items, and match rules.
4. **Networking:** connection lifecycle, protocol messages, replication, prediction support, and downloads if retained.
5. **Client presentation:** `wgpu` rendering, a custom HUD and menus, `cpal`-backed game audio, and effects consuming game state rather than owning authoritative gameplay.
6. **Game rules and tools:** Rhai-based gametypes and map hooks, configuration, bots, and `egui` developer utilities behind stable interfaces.

The authoritative dedicated server owns decisions that affect match outcomes. The client predicts its own movement and reconciles against authoritative server state; remote players are interpolated from server updates. Server-confirmed state remains the source of truth. Isolate protocol encoding and decoding behind an interface so gameplay and transport can evolve independently, even though legacy wire compatibility is not required. Keep `bevy_ecs` behind engine-owned game-facing interfaces so gameplay rules and content do not become coupled to Bevy's application framework.

## Rendering pipeline decisions

The renderer should be designed around the actual workload: a first-person arena game with BSP worlds, moving players and projectiles, frequent transparency/effects, and a target on an RTX 2080-class GPU of holding 144 FPS in typical combat, with 1% lows near 120 FPS, at 1080p/high settings without ray tracing. The initial minimum API tier is modern hardware supporting Vulkan 1.2, DirectX 12, or Metal; older OpenGL-era hardware is out of scope unless later added deliberately. `wgpu` supplies portable GPU access, but does not choose the renderer architecture or guarantee every optional GPU feature on every target.

### Starting direction

- **Rendering model:** start with a forward renderer, with material and light batching designed so it can grow into forward+ or clustered lighting if profiling shows many dynamic lights are costly. Forward rendering is a straightforward fit for BSP surfaces, transparent effects, and multisample anti-aliasing. A deferred renderer can make many opaque dynamic lights cheaper, but adds render targets/bandwidth and complicates transparency and MSAA; do not choose it without a measured need.
- **World visibility and submission:** use BSP visibility data where available to find potentially visible surfaces, then frustum-cull and batch visible surfaces by material. Keep CPU-side visibility/culling initially, and measure submission cost before moving visibility work to compute/GPU. Preserve BSP surface and cluster identifiers for debugging and future optimization.
- **Static and dynamic lighting:** first support the map's baked lighting/lightmaps, then add dynamic lights as a separate feature. Keep lightmap sampling and surface material data explicit so lighting can be validated independently of the rest of the renderer.
- **Transparency and effects:** render opaque world geometry first with depth testing/writes, then order transparent geometry and particles back-to-front where blending requires it. Add a distinct path for effects that need additive blending or depth testing without depth writes. Avoid requiring order-independent transparency in the initial slice.
- **Anti-aliasing:** begin with MSAA if supported by the selected color/depth formats and render path; confirm its cost and visual results on target GPUs. Keep post-process anti-aliasing as a later alternative if MSAA is too costly or incompatible with desired effects.
- **Color and presentation:** render to an sRGB surface or an explicitly managed linear/HDR intermediate target, and make tone/color conversion a deliberate step. Start with SDR output; HDR display support is a separate scope decision.
- **Frame organization:** use explicit render passes and resource ownership. Add a small render graph only when dependencies, transient targets, or pass reordering justify it; avoid building a general-purpose graph before the first map renders.

### Choices to validate in the vertical slice

| Decision | Options and tradeoff | Initial validation |
|---|---|---|
| Surface format and presentation mode | Surface formats, vsync/FIFO, and lower-latency modes vary by platform and display. | Select a supported SDR format and vsync default; report actual selected mode and provide a fallback when a preferred mode is unavailable. |
| Depth and MSAA | Depth format and sample count affect compatibility, bandwidth, and edge quality. | Verify a depth-tested BSP scene with MSAA on representative Windows, Linux, and macOS hardware; make sample count configurable for testing. |
| BSP visibility | PVS/cluster filtering can reduce draw work, while frustum culling removes off-screen geometry; a simple all-visible path is easier to validate. | Render one map correctly with a baseline path, then enable PVS and compare visible surfaces and frame cost. |
| Lightmaps and material representation | Existing BSP lighting/material conventions may need format-specific interpretation. | Inspect representative map data and verify lightmap UVs, texture addressing, color space, and brightness against reference screenshots. |
| Dynamic lights | Per-light forward draws, forward+ clustering, or deferred lighting have different CPU/GPU costs. | Begin with few lights and profile realistic effects before selecting a scalable lighting path. |
| Transparency and particles | Sorting is simple but can be costly or imperfect; specialized techniques add complexity. | Validate common transparent surfaces, weapon effects, and particles with depth and blending behavior. |
| Shader variants | Static pipelines are predictable; variant proliferation increases compile and maintenance costs. | Start with a small set of explicit material pipelines and measure shader compilation/stutter before expanding variants. |
| Resolution scaling and post-processing | Native resolution is simple; intermediate targets enable scaling and post effects at added bandwidth/cost. | Defer post-processing and dynamic resolution until the base scene is correct and GPU timing is available. |

### Renderer boundaries and observability

Keep map parsing and gameplay entities independent of GPU resource types. A content-to-render conversion stage should produce renderer-owned meshes, textures, materials, and lightmap resources; simulation should expose snapshots or render data rather than issuing GPU commands. Keep shader/material definitions versioned and diagnosable, and make asset failures visible instead of silently substituting content.

Instrument CPU time for visibility, batching, and command encoding separately from GPU pass timings. Provide debug views for BSP clusters, surface/material IDs, lightmaps, depth, and overdraw where practical. Validate representative scenes against the 144 FPS typical-combat / 120 FPS 1%-low target at 1080p, high-quality/no-ray-tracing settings on RTX 2080-class hardware and across all supported desktop backends before committing to a more complex pipeline.

## Port strategy

### 1. Establish requirements and a behavior reference

- Confirm target platform requirements and runtime constraints.
- Decide which existing maps, assets, scripts, and demos must remain compatible.
- Record per-area decisions, content-root precedence, dependencies, and reference scenarios in [PORT_COMPATIBILITY.md](PORT_COMPATIBILITY.md).
- Inventory gameplay rules and map/entity behavior from the current engine.
- Capture representative scenarios and expected outcomes: connect, load a map, spawn, move, use each weapon, pick up items, score, transition match state, and change maps.
- Establish baseline build and test commands for the current engine where feasible.

**Exit condition:** compatibility decisions are written down, the content roots and precedence are verified, and the reference scenarios can be repeated or compared.

### 2. Prove the offline vertical slice

Build the smallest end-to-end implementation that demonstrates the chosen target stack:

- Start the desktop runtime and load one representative existing BSP map directly.
- Parse its world and the entities needed by the slice, and provide working collision.
- Spawn a local player and match the reference movement cases, including core air control and special movement, before adding one weapon and item pickup.
- Render and present the map and player state, with basic sound if needed.
- Add debug tools to inspect map entities, collision, and player state.
- Run recorded movement inputs through the reference and port, comparing per-step state against documented tolerances.

Prefer a deliberately small slice over scaffolding every subsystem. It should exercise real content and a real gameplay loop, without requiring networking.

**Exit condition:** the offline slice described above runs on a target platform and its behavior can be compared against the reference scenarios.

### 3. Expand by subsystem

Replace or implement the remaining systems behind the established interfaces. Keep each change bounded and testable. Preserve the deterministic movement regression suite as simulation changes and multiplayer are added; verify that authoritative server movement and client prediction agree for the same inputs and collision state. After the offline slice, add authoritative dedicated-server simulation and the new custom UDP client/server protocol, including reliability, ordering, and congestion handling appropriate for real-time play. The first multiplayer release contains Bomb and Defuse only; defer listen servers and other modes. Then broaden presentation, audio, tools, and bots. Port required map and gameplay behavior to internal Rhai scripts incrementally; AngelScript compatibility and user scripting are not initial requirements.

Where feasible, compare old and new implementations against the same maps and scenarios. Track intentional differences explicitly instead of silently accepting behavior drift.

**Exit condition:** each required feature has a validation scenario and a documented compatibility status.

### 4. Migrate content and retire legacy paths

Validate the required map, asset, script, and configuration corpus. Add conversion tools only where direct loading is not practical, and make conversions reproducible. Remove legacy implementations only after the replacement meets agreed compatibility requirements and has adequate test coverage.

**Exit condition:** required content works, unsupported content is identified, and removal of each legacy path is deliberate.

## Validation

Maintain automated or repeatable checks in several layers:

- **Format tests:** valid and malformed BSPs, entity strings, and content metadata.
- **Simulation tests:** movement traces, collision outcomes, weapon behavior, pickups, and scoring.
- **Determinism tests:** replay identical input traces at 125 Hz across supported operating systems and compare state hashes or documented numeric tolerances.
- **Server lifecycle tests:** connection and rejection paths, map load/change, respawn, match transitions, disconnects, and timeouts.
- **Network prediction tests:** compare predicted local movement with authoritative server movement, including reconciliation after packet delay/loss; verify remote-player interpolation separately.
- **Network impairment tests:** exercise Bomb and Defuse under up to 200 ms RTT and 3% packet loss, including the 75 ms lag-compensation cap.
- **High-frequency protocol tests:** sustain the 125 Hz snapshot target under representative player/entity counts and measure bandwidth per client, packet loss, CPU cost, correction frequency, and interpolation quality.
- **Compatibility scenarios:** representative existing maps and gametypes, compared with the reference implementation where possible.
- **Platform checks:** startup, input, rendering, audio, filesystem behavior, and packaging on each supported target; profile representative scenes against the RTX 2080-class target (144 FPS typical combat, 120 FPS 1% lows) at 1080p high-quality/no-ray-tracing settings.

For behavior comparisons, define tolerances for values that can vary by timing or floating-point implementation. Keep exact requirements—such as score, match result, entity identity, or protocol fields—separate from approximate ones such as visual output or movement trajectories.

## Risks and decisions to resolve

- **Protocol compatibility:** the new protocol requires coordinated client/server deployment and will not work with existing qfusion/Warsow clients or servers. The offline first slice must not be mistaken for proof that multiplayer networking is solved.
- **Content compatibility:** map formats may rely on implicit entity or collision semantics not obvious from the file format alone.
- **Movement fidelity:** small physics or collision changes can substantially change competitive gameplay; a third-party collision crate must pass reference movement traces before adoption.
- **Cross-platform determinism:** floating-point behavior and collision-library differences can cause prediction or replay divergence across operating systems.
- **Script compatibility:** existing AngelScript hooks and game modes may encode behavior that is not captured in high-level descriptions.
- **Scope growth:** attempting a full feature port before the vertical slice is validated makes regressions and schedule risk difficult to manage.
- **Remaining stack:** choose concrete Rust crate versions and the import/conversion pipeline based on the vertical slice and target constraints rather than independently.

Before implementation begins, record the selected map, asset, configuration, and demo compatibility requirements, then select concrete dependencies for content import and runtime loading. The 125 Hz snapshot target is independent from the 125 Hz simulation tick; verify both independently with multiplayer profiling and impairment tests.

Other decisions intentionally deferred until prototypes provide evidence include the exact supported legacy map/asset set, output asset formats, final visual direction, Rust crate versions, and detailed packet encoding, baseline acknowledgment, and congestion/adaptation policy.
