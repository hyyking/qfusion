# Recording and comparing your movement with the original engine

`qf-client` can record your run. `qf-movecmp` replays that recording through the Rust engine
and through the original `gs_pmove.cpp` (on the same wbomb1 BSP) and reports where and why
they diverge. The legacy trace harness is described in `movement-reference.md`.

## 1. Record your run

```
cargo run -p qf-client --release -- "C:\Program Files (x86)\Warsow 2.1\basewsw\map_wbomb1.pk3"
```

* **F9** starts a recording; **F9** again stops it and saves. A red dot with `REC <ticks>`
  shows in the top-left corner while recording; `REC SAVED` / `REC FAILED` appears for four
  seconds afterwards, and the path is printed on stdout. Closing the window or Shift+Esc also
  saves an active recording.
* The file is `qf-recording-<unix seconds>.qfrec` in the current directory. Use
  `--record my-run.qfrec` to start recording at launch and save to that file.
* Recording needs walk mode (the client starts in it). If the player is not walking, the HUD
  shows `REC NEED WALK`. Leaving walk mode stops and saves the recording.
* Recording only copies each 125 Hz tick's command and result, so it never changes the
  simulation.

## 2. Compare with the original engine

Build the legacy replay tool once (needs a C++ toolchain; CMake only builds the game-module
sources it needs):

```
cmake -S source -B build-pmove -DGAME_MODULES_ONLY=ON -DSERVER_ONLY=ON -DQFUSION_GAME=Warsow
cmake --build build-pmove --target pmove_replay --config Release
```

The executable is `source/build/<config>/pmove_replay_<arch>` (a platform suffix is added).
Then:

```
cargo run -p qf-engine --release --bin qf-movecmp -- compare ^
    --recording my-run.qfrec --legacy-exe <path-to-pmove_replay> --out-dir cmp
```

The map pack is found as `map_<name>.pk3` in `%WARSOW_BASEWSW%` (default
`C:\Program Files (x86)\Warsow 2.1\basewsw`); pass `--map <pk3|bsp>` to override. The tool
refuses to run if the BSP differs from the one recorded (size and FNV-1a hash).

Exit status: `0` match, `1` divergence, `2` error. Output in `--out-dir`: `report.md`
(summary, first divergence, error growth, cause), `comparison.csv` (every tick, both sides),
`legacy.qfrec` (the legacy run in the same format), `map.bsp`, and on divergence
`rust-trace.txt` / `legacy-trace.txt` (collision queries at the divergence tick).

Other modes:

* `--rust-only` replays through the Rust engine and checks it reproduces what the client
  recorded (determinism check).
* `--legacy-recording legacy.qfrec [--legacy-trace legacy-trace.txt]` compares against an
  earlier legacy output without needing the executable.
* `--tol-position N`, `--tol-velocity N` (default 0.05 units, units/s) and `--window N`.

## Tolerances and what is compared

Exact: grounded state, pm flags, pm_time, water level, ladder, step presence, crouch hull
height, movement events (id and parameter) and touched entities. Within tolerance: origin,
velocity, view height and step height (0.05; the legacy dash uses an `rsqrtss` approximation
worth about 0.03). Delta angles are not compared (legacy `Pmove` leaves them to the game
module). Trigger ticks (jump pads, teleporters) are resynced from the recording on both sides
because the legacy movement code has no entity system.

### Cause classification

At the first divergent tick, the Rust and legacy collision queries (box/point traces) are
compared: same queries with different results is a **collision** mismatch (BSP differences,
e.g. the Rust world is brush-only while the legacy model also has patches); different query
sequences is a **control-flow** mismatch; identical queries and results with different state
is a **movement-integration** mismatch.

## File format (version 1)

Text, one value per line in the header, then one CSV row per tick.

```
qfusion-movement-recording 1
map_name / map_entry / map_size / map_fnv1a64 (hex)
tick_rate_hz  touch_hook  generator
start_origin / start_velocity / start_view_angles / start_view_height / start_mins /
start_maxs / start_flags / start_pm_time / start_gravity / start_delta_angles / stat_<name> x14
ticks N
tick_columns tick,msec,buttons,forward,side,up,angle0,angle1,angle2,x,y,z,vx,vy,vz,grounded,
             flags,pm_time,view_height,maxs_z,delta0,delta1,delta2,water_level,ladder,step,
             trigger,events,touches
<N rows>
```

The first columns (`msec` .. `angle2`) are the user command fed to the engine; the rest is the
resulting state. Floats are written in shortest round-trip form, so reading a file back is
lossless. `events` is `id:param;...` with the legacy numeric ids (Dash 9, WallJump 10,
WallJumpFailed 11, DoubleJump 12, Jump 13, JumpPad 14, Fall 15); `touches` is `a;b`.
`touch_hook 1` means the client ran with map entities (the trigger-aware path, which also
refreshes delta angles after each move). The version number changes on any incompatible edit.

## Sample session

`rust/qf-engine/tests/fixtures/wbomb1_sample.qfrec` is a scripted 876-tick run on wbomb1
(settle, run, strafe, turn, jump, crouch, dash, double jump, ...). Regenerate with
`qf-movecmp sample --map <pk3> --out FILE`. `wbomb1_sample.legacy.qfrec` is the output of
`pmove_replay` on it. `cargo test -p qf-engine --test movement_compare` checks the Rust replay
is bit-exact and matches the legacy output (the real-map test skips if the pack is missing).

Result for the sample: no tick outside tolerance; maximum position error 0.0000 and maximum
velocity error 0.0002 over 876 ticks.

## Limits

* Interactive recordings are only comparable where the legacy and Rust collision agree; the
  Rust `BspWorld` has no patch collision or submodels (movers), so runs touching those can
  report collision divergences. These are reported, not changed.
* V (noclip toggle) is not wired in this client; noclip ticks are not recorded.

## Per-step mode and known deviations

Continuous replay accumulates sub-tolerance drift and diverges chaotically once any tick exceeds tolerance. Pass `--per-step` to `qf-movecmp compare` to restart the legacy engine each tick from the previous recorded state, which isolates single-step fidelity. Trigger ticks (jump pads, teleporters) copy the recorded events/touches into both replays.

Findings from a 15336-tick wbomb1 session:

- Dash ticks differ by 0.10-0.14 u/s in velocity: the legacy horizontal normalisation uses an approximate reciprocal square root. Reported, reference semantics unchanged.
- A zero-length box trace flush against a wall is start-solid in the legacy position test; the Rust trace now implements that special case (`Bsp::trace` with `start == end`). Recordings made before this fix no longer replay bit-exactly at the affected ticks.
