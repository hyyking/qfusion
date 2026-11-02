# Unifying `GameHost` and `Sim` (plan)

Status: **plan for decision**. Follow-up to `docs/ECS_AND_SCRIPTING.md` (Option B, migration
steps 1-10 landed). No code accompanies this document.

## 1. Where we are

Two ECS worlds hold "a player":

| | `game::roster::Roster` (inside `GameHost`) | `simulation::entities::MapEntities` (inside `Sim`) |
|---|---|---|
| Player data | `GamePlayer(Player)`: team, alive, health, armor, origin, velocity, yaw, pitch, `Inventory`, `stats` | walk state (`WalkPlayer`), `Alive`, `Inventory`, `WeaponCmd`, pending usercmd |
| Driven by | script hooks; the host/client pushes origin/velocity via `set_player_motion` | `Sim::tick` (pmove, weapons) |
| Authoritative for | rules (health/armor via `Player::take_damage`, team, stats) | motion, fire/ammo, projectiles |
| Hashed | `GameHost::state_hash` (Debug string) | `Sim::state_hash` (FNV, field by field) |

They meet only through glue: the caller copies motion into the roster, and
`GameHost::apply_weapon_events` turns `SimEvent::Weapon(Damage)` into `on_damage`. Consequences:
health/armor/alive exist in two places, `WeaponEvent::Damage` cannot express world damage
(`attacker: usize`), blasts (`radius_damage`) go straight to `on_damage` and not through the
weapon path, Bomb state is an opaque `ScriptState` map, and `GameHost::tick` runs its own map
world (`MapEntities` again) next to the `Sim` one.

## 2. Target

One world, one tick, one hash.

- `GameHost` owns a `Sim` (single `MapEntities` world). `Roster` disappears; its players are
  `Sim` player entities.
- Player components (split `Player`): `Slot`, `TeamC`, `Alive`, `Health`, `Armor`, `Transform`
  (origin, velocity, angles; written by pmove only), `Inventory`, `WeaponCmd`, `Stats`
  (BTreeMap, script-owned).
- Rules stay in Rhai. Rhai reads/writes components through the existing `g.*` API; the engine
  applies mutations at defined points (command-buffer style), never mid-system.
- Damage has one path: every source (weapon hit, blast, trigger_hurt, script) emits
  `SimEvent::Damage { victim, attacker: Option<Slot>, cause, amount }`. The engine hands the
  event to the script's `on_damage` (which may scale/deny/convert), then applies the resulting
  health/armor change to the components and emits `Died` when it reaches zero.
- Bomb state moves from `ScriptState` to components/resources only where the engine benefits
  (see PR D): `Bomb` entity (state, carrier, origin, timers), `BombSite` entities, `Timer`
  component. Round/score flow stays script state.

## 3. The ordering concern (why this is not a small PR)

Today `bomb.rhai` applies blast damage synchronously inside `explode()`; `on_damage` may call
`die`, which can end the round and emit events *in that call*. Moving damage to an event queue
changes when those happen. Rule for the migration: **a damage event is dispatched to
`on_damage` immediately and in slot order at the point it is raised** (a synchronous
"event with a handler", not a deferred queue), and the event is also published to
`SimEvent` for audio/HUD/replay. This reproduces the current event order exactly, so same-seed
fingerprints stay equal. Deferring to end-of-tick is a separate, deliberate behaviour change
(own PR, golden hashes regenerated, listed in the description).

## 4. Options

- **A. Merge worlds, keep `Player` struct as one component.** Smallest diff, but health is then
  not independently queryable/hashable and `Sim` systems would need the rules' `Player`.
  Rejected: keeps the god-component.
- **B. Merge worlds and split `Player` into components (recommended).** Matches the design doc;
  the work is mechanical per component and each step is verifiable by the fingerprints.
- **C. Keep two worlds, sync via events.** Cheapest, but is exactly the duplication we want to
  remove and blocks prediction/networking. Rejected.

## 5. Ordered PRs (each based on master, each keeps bomb determinism, replay, pmove traces)

Verification oracle for every step: synthetic Bomb scenarios, `wbomb1` `fingerprint` equality
before/after, replay checkpoints, the pinned golden hashes in `bomb_synthetic.rs`
(regenerated *only* in a PR that says it changes behaviour), pmove trace harness.

1. **PR A - `Sim` owns the map world used by `GameHost`.** `GameHost` holds a `Sim` instead of
   its own `MapEntities`; `run_map` becomes `Sim::tick`'s map step. No player change yet.
   Hash: `GameHost::state_hash` mixes `Sim::state_hash`.
2. **PR B - players in the `Sim`.** `Roster` slots become `Sim` player entities with the
   existing `GamePlayer(Player)` component plus the walk/weapon components on the same
   entity. `set_player_motion` becomes a write to `Transform`; the client/headless harness
   feeds `usercmd`s instead (pmove runs inside `Sim`). Headless tests that teleport players keep a
   test-only `teleport` API. Removes `apply_weapon_events` glue (the weapons system and the
   hook run in one tick).
3. **PR C - split `Player` into components and make them authoritative.** `Alive`, `Health`,
   `Armor`, `TeamC`, `Stats` as components; `Player` survives as a *view* struct for the
   `g.*` API/tests. `take_damage` becomes a system helper. State hash covers each component
   explicitly (replaces the Debug-string hash; golden hashes regenerated, behaviour unchanged,
   fingerprints equal).
4. **PR D - unified damage + `radius_damage` through `SimEvent`.** Implements section 3:
   `SimEvent::Damage` with optional attacker and cause; weapon hits, blasts and
   `trigger_hurt` all use it; `g.radius_damage` becomes the engine blast (victim selection
   already engine-side since #50). bomb.rhai `on_damage` signature gains `cause`
   (default-compatible for old calls). Tests: ordering identical to master for the
   explode scenario; world damage has no attacker.
5. **PR E - Bomb state to components.** `Bomb`, `BombSite`, `Timer`; bomb.rhai reads via
   handles (`g.bomb()`, `g.site(i)`), `this.bomb` map shrinks to round flow. Done as a
   behaviour-preserving port; the headless scenarios (plant/explode, defuse, timeout,
   eliminations, role swap, score limit) are the oracle. Risky, so split by field group if the
   diff is large (timers first, then carrier/drop state, then site state).
6. **PR F - cleanup.** Delete `Roster`/adapters, handle types and cached entity indices for
   script-facing entities, pickups in `Sim` (the `Pickups` rules from `game/pickups.rs` become a
   system on player + item entities, matching `simulation::entities` pickup touch kinds), update
   `SUMMARY.md`, `ENGINE_PORT.md`, `BOMB_AND_DEFUSE.md`.

PRs A and B are the riskiest (they move the owner of motion). If `set_player_motion` callers
(client, tests) make B too large, B can be split: B1 adds the components next to `Roster` and a
mirror check test, B2 flips authority.

## 6. Risks

- **Motion ownership.** Today the host trusts caller-supplied motion; after B, `Sim` runs pmove.
  The movement-comparison tools and `qf-client` already own `WalkPlayer`; they must feed
  usercmds. The pmove trace harness is untouched because pmove stays a pure function.
- **Hash churn.** Replacing the Debug-string hash changes all golden values once (PR C);
  that PR must show unchanged event fingerprints as proof.
- **Script API stability.** Keep every existing `g.*` accessor; only add. Remove after the
  Bomb port no longer uses them.
- **Replay logs.** `Op::Motion` is recorded input today; after B it becomes `Op::UserCmd`.
  Bump the recording format version in B and keep a loader for v1 only if needed.
- **Rhai performance.** Per-call component lookups go through the cached slot->entity index
  (PR F makes this a handle); budget limits (2M ops/hook) unchanged.

## 7. Defaults chosen / open questions

1. Damage is dispatched synchronously at raise time (section 3). *Default: yes.*
2. Round/score flow stays in script `ScriptState`; only Bomb objects become components.
   *Default: yes.*
3. `Player` remains as a read view struct for tests and the `g.*` API. *Default: yes.*
4. Headless tests keep a `teleport` helper instead of simulating walking. *Default: yes.*
5. Recording format bumps to v2 in PR B (inputs are usercmds). *Default: yes, no v1 loader.*
6. Blast `cause` values: `weapon`, `blast`, `world`, `script`. *Default: as listed.*

Please confirm or redirect these before PR A starts; PRs A-C do not depend on 1, 2 and 6.
