# Legacy movement reference harness

The harness calls the engine's existing `Pmove` function in
`source/common/facilities/gs_pmove.cpp`. It does not duplicate movement math or
read render frames. Its collision callback supplies a flat floor and, where a
case needs one, an axis-aligned wall; water and ladder cases use the same
callback interface as the engine. These fixtures are deliberately small and
are not BSP maps.

## Build and run

The harness compiles `Pmove`, its collision response, and required math/vector
implementation files directly from the repository. It does not need the
engine's Qt or third-party runtime dependencies. From the repository root:

```sh
cmake -S source -B /tmp/qfusion-pmove-build -DGAME_MODULES_ONLY=ON -DQFUSION_GAME=Warsow
cmake --build /tmp/qfusion-pmove-build --target pmove_trace -j"$(nproc)"
./source/build/pmove_trace."$(uname -m)" > /tmp/pmove.csv
./source/build/pmove_trace."$(uname -m)" --rate 125 --case jump_tap > /tmp/jump.csv
```

For the full legacy engine, use the README build flow and data requirements:

```sh
cd source
cmake -DQFUSION_GAME="Warsow" -DCMAKE_PREFIX_PATH=/opt/qt/5.13.2 .
make -j"$(nproc)" warsow wsw_server
```

Run the built client from a directory containing `basewsw`, as described in
`README.md`; from the repository root on Linux x86_64 the executable is
`source/build/warsow.x86_64`.

Omitting options runs every case at each supported nominal rate: 125 Hz (8 ms
commands), 100 Hz (10 ms), 62.5 Hz (16 ms), 60 Hz (17 ms), 50 Hz (20 ms), and
30 Hz (33 ms). The engine's command duration is an integer number of
milliseconds, so 60 Hz and 30 Hz use the nearest whole-millisecond duration;
the trace records that duration rather than assuming a fractional timestep.
Use `--rate` and `--case` to select a single combination. Available cases are
`jump_tap`, `jump_hold`, `air_direction_change`, `dash_wall_hit`,
`walljump_after_dash`, `land_slow`, `land_fast`, `water_swim`, and
`ladder_climb`.

Each command is sampled once, at the beginning of its simulation interval, and
its input is selected from simulation elapsed milliseconds. No render clock,
frame callback, or asynchronous input sampling participates. To check
repeatability with the same binary and platform:

```sh
./source/build/pmove_trace."$(uname -m)" > /tmp/pmove-a.csv
./source/build/pmove_trace."$(uname -m)" > /tmp/pmove-b.csv
cmp /tmp/pmove-a.csv /tmp/pmove-b.csv
```

CSV rows contain scenario, nominal rate, step index, simulation time, command
duration, input buttons/movement/angles, position, velocity, grounded state,
water level, ladder state, raw movement flags, predicted events, touched entity
IDs, and step height. This records the actual command sequence supplied to
`Pmove`. Vectors and step height are decimal floats printed to six places.
`events` contains
the event name, numeric event ID, and exact integer parameter for each event
emitted during that step; `touches` contains entity numbers from the movement
trace. Input sampling occurs at each row's `time_ms` and uses the selected
scenario's fixed schedule.

## Comparison contract

Compare these as **exact invariants** for a fixed harness build and case:

* The simulation step count, command duration, input schedule, and event
  sequence (event name and parameter).
* Grounded, water-level, and ladder state, plus movement-flag transitions
  including jump-held, dashing, wall-jumping, and crouch-sliding bits.
* The ordered touch-entity list and whether a step-height result is zero.

Compare position, velocity, and nonzero step height as **float-tolerance
values**, not bitwise cross-platform values. Use an initial tolerance of
`0.05` engine units per component for same-compiler reference comparisons;
increase only with a documented compiler/platform investigation. CSV values
are rounded for display, so compare unrounded internal values if building a
test that enforces tolerances. This first harness does not claim cross-platform
bitwise determinism.

## Captured movement rules (code cross-check)

The values below describe the legacy implementation, not a proposed movement
rewrite. The main entry point runs setup, categorization, special movement,
jump, dash, wall-jump, crouch-slide, friction, acceleration, collision, and
final categorization in a fixed order (`Pmove`, `PM_Move`).

* **Player shape and gravity:** standing player bounds are `(-16,-16,-24)` to
  `(16,16,40)` and crouched bounds are `(-16,-16,-24)` to `(16,16,16)`;
  view heights are 30 and 12 (`gs_public.h`). The game gravity constant is 850
  against a base of 800. The move state supplies the gravity value consumed by
  `PM_Move`; normal game setup is responsible for initializing it.
* **Ground movement and friction:** normal ground acceleration is 12 and
  ground friction is 8, with deceleration control 12 (`gs_pmove.cpp` movement
  parameters, `PM_Accelerate`, `PM_Friction`). Friction is omitted on slick
  surfaces and while knockback is active. Walk speed is 160; crouched speed is
  100, both capped relative to the configured player speed. Ground acceleration
  adds velocity only along the wish direction, up to the remaining wish speed.
* **Air movement and strafe-bunny:** air acceleration is 1 and air
  deceleration is 2. With forward-bunny enabled, `PM_AirAccelerate` uses a
  1.00001 forward acceleration factor until max speed, then a 0.1593 soft
  acceleration toward a 925 speed ceiling; turn acceleration is 4 and the
  backward-to-side ratio is 0.8. The strafe-bunny path caps wish speed at 30
  and uses acceleration 70. The separate air-control path rotates horizontal
  velocity but does not permit changing direction while slowing down. Which
  branch runs depends on the feature bits.
* **Jump:** `upmove >= 10` requests a jump. Without continuous-jump enabled, a
  jump must be released before it can be requested again (`PMF_JUMP_HELD`).
  Water level 2 or 3, non-normal movement, missing jump feature, and being
  airborne prevent a regular jump. Launch speed is the configured jump speed
  multiplied by `850/800`; upward speed above 100 yields `EV_DOUBLEJUMP`,
  otherwise `EV_JUMP`. Jump clears dash and wall-jump state.
* **Dash and wall jump:** special-button dash starts only while grounded, with
  dash enabled and no knockback. It sets a 1000 ms timer and chooses the dash
  direction from forward/side input, defaulting to forward when neutral. A
  wall jump is requested in air with special held, once its timer/count permits;
  it tests nearby collision directions, rejects sky and `SURF_NOWALLJUMP`, and
  uses a 1300 ms successful or 700 ms failed cooldown. A dash prevents a wall
  jump during its first 100 ms. Wall and ground collision response still runs
  through the normal slide movement.
* **Crouch and crouch slide:** crouch dimensions/viewheight transition over
  100 ms, and standing back up is deferred if the expanded box is obstructed.
  A crouch slide starts on a crouch/down command while airborne and above walk
  speed; it has a 1500 ms active duration plus a 500 ms fade. It suppresses
  ground friction during the slide, scales slide acceleration by 3, and caps
  the resulting speed as implemented in `PM_Accelerate`.
* **Gravity, ground, slopes, and steps:** airborne movement applies
  `gravity * frametime` to vertical velocity. Ground categorization traces
  one quarter unit down and accepts walkable planes; upward velocity above 180
  clears grounded state. `PM_SlideMove` clips velocity against up to five
  collision planes using overbounce 1.01, and `PM_StepSlideMove` compares a
  direct slide with a step-up/slide/down path using `STEPSIZE`. Walkable slopes
  preserve horizontal speed; non-walkable planes block walking.
* **Water and ladders:** water level is sampled at three heights, with water
  friction 1 and acceleration 10. Swimming wish speed is halved after the
  normal max-speed cap, and water currents and water-jump checks are separate
  paths. A ladder is detected by a one-unit forward trace hitting `SURF_LADDER`;
  ladder vertical input uses the configured ladder speed, and gravity is
  countered toward zero when there is no vertical input.
* **Landing and event/touch output:** landing is detected by the change from
  airborne to a walkable ground trace. `EV_FALL` is emitted when fall-speed
  change exceeds 400; the damage calculation begins above 675 and is disabled
  or scaled by water, surface, and jump-pad state. Collision contacts are
  recorded in `pmove_t::touchents`, while the game callback handles trigger
  touches after movement.

Relevant implementation locations are `gs_pmove.cpp` (movement constants and
helpers near the top; friction/acceleration and `PM_Move`; jump/dash/wall-jump
checks; `PM_CategorizePosition`, `PM_SlideMove`, and `PM_StepSlideMove`) and
`gs_public.h` (player box dimensions and default speeds).

## Coverage limits and in-game cross-check

The traces include tap-versus-held jump, a mid-air direction change, a dash
into a wall, a wall-jump attempt after dash release, slow/fast landings, water,
and ladder fixtures at each rate above. Collision surfaces in this harness are
simple axis-aligned test planes; it does not load BSP maps and does not model
sloped brushes, step geometry, moving platforms, contents transitions, or all
game trigger logic. For those cases the harness must be extended with recorded
BSP/collision fixtures before its results can serve as a complete movement
acceptance gate.

The independent Rust BSP collision tests are under `tools/bsp-poc`: they check
parsing, brush traces, and trace outputs without running `Pmove`. Keep those
results separate from movement traces so a collision mismatch is distinguishable
from movement integration; this does not add a Rust movement implementation or
a live map-driven `Pmove` comparison.

No in-game session or reference BSP assets are available in this repository
checkout, so live behavior has **not** been independently verified here. To
complete that cross-check, run the same command sequence and map in a legacy
Warsow client/server build, record per-command origin/velocity/ground state and
events, and compare the exact invariants and float tolerances above. Do not
change legacy movement semantics merely to force a deterministic match.
