# Bomb and Defuse (offline, headless)

The gametype lives in `scripts/gametypes/bomb.rhai`; the engine only provides a generic,
map-agnostic host (`game::GameHost`). The script owns every rule and discovers spawns and
bomb sites from the map's entities by classname, so it is not tied to `wbomb1`.

## Architecture

- `game::host`: embeds Rhai. Hooks the script defines: `init(g)`, `tick(g)` (required);
  `start_match`, `on_player_joined`, `on_team_change`, `on_command`, `on_damage`, `hud`
  (optional). State lives in the script's `this` map. Fixed 125 Hz (`tick()` advances 8 ms),
  seeded RNG, `BTreeMap` everywhere, no wall clock: the same seed and inputs give the same events.
- Script API (`g.*`): clock/rng, `entities()`, `trace`/`los`/`in_world`, player queries and
  mutation (`spawn_at`, `apply_damage`, `ghost`, health/armor/stats, weapons),
  `set_movement_locked`, `emit(kind, map)`. Events go to the client/server/tests via `take_events()`.
- `game::combat`: `Player`, `Team`, the armor rule (absorbs 66%, degrades by the same amount).
- Client (`qf-client::bomb`): runs the host beside the walking player, teleports on `spawn`
  events, locks movement during the round countdown / when dead, draws the HUD from
  `GameHost::hud`. Run: `qf-client <pk3> --entry maps/wbomb1.bsp [--team alpha|beta] [--freeplay]`.
  Keys: G drop bomb, K suicide, T switch team.

## Legacy behavior inventory (`source/server/game/progs/gametypes/bomb/*.as`)

| Area | Ported behavior |
| --- | --- |
| Match | countdown (5 s) → rounds → postmatch. Warmup is skipped. Score limit 16, win by 2. |
| Round | pre 5 s (movement locked, speed 100) → round 60 s → finished 1.5 s → post 3 s. |
| Teams/roles | Attackers spawn at `team_CTF_betaspawn`, defenders at `team_CTF_alphaspawn` (legacy picks by role). Alpha attacks first. Roles swap using the legacy `swap_roles` formula, quirks included. |
| Carrier | One random attacker gets the bomb each round. Drop (command, death, team change), toss physics, 1 s no-retake for the dropper, pickup by any alive attacker. Bomb out of the world → defenders win. |
| Plant | Carrier on ground, speed ≤ 400, inside a `trigger_capture_area` (AABB) of a site. Bomb is placed on the floor under the carrier. Arm time 4 s: progress needs an alive attacker within 100 units with line of sight, otherwise it decays. |
| Explosion | 30 s after arming. Flat 9001 radius/damage kills everything with LOS, then the site's `target_explosion`/`func_explosive` entities fire with their delays (radius + linear falloff + LOS). Attackers win the round. |
| Defuse | Same progress rule with defenders, 7 s. Checked before the explode timer. Defenders win. Warning at 12 s before the blast. |
| Win conditions | attackers dead and bomb not armed → defenders; defenders dead → attackers; round timeout without an armed bomb → defenders; defuse → defenders; explosion → attackers (scored once). |
| Loadout | health 100, armor 100, gunblade + primary (default RL+EB, choose 2 of rl/lg/eb) + secondary (default PG; pg/rg/mg/gl/gb). Ammo as legacy. Firing lives in `simulation::sim::Sim` (single fire mode per weapon, legacy "strong" values): player entities carry `Inventory`/`WeaponCmd`/`Alive`, a `Weapons` system runs after `Movement` in the 125 Hz schedule (rules in `game::firing`), projectiles are entities, and shots/impacts/damage are `SimEvent::Weapon`; `GameHost::apply_weapon_events` routes `Damage` through `on_damage`. The client is not on `Sim` yet, so it has no weapon input/HUD. |
| Damage | Armor rule above. Teamdamage during spawn protection (3 s) is reflected (×2 first third, ×0.5 last third; armor −dmg/2, health −dmg). |
| Items | Disabled in the mode (`spawnableItemsMask = 0`); item entities on wbomb1 are ignored. |

Local-play rule (documented deviation): a team only counts as eliminated if it had players at
round start, so a lone attacker can plant and a lone defender can run out the clock.

## Entity support ledger (wbomb1, bomb mode)

| Entity | Status |
| --- | --- |
| `team_CTF_alphaspawn` / `team_CTF_betaspawn` | Used (defender / attacker spawns) |
| `misc_capture_area_indicator` | Used (bomb site, floor-dropped unless spawnflag 1) |
| `trigger_capture_area` | Used (site plant volume, submodel AABB) |
| `target_explosion`, `func_explosive` | Used (site blast, delays/radius/dmg) |
| items / weapons / ammo / health / armor | Ignored on purpose (mode disables items) |
| `target_location` | Not implemented (no callouts) |
| `target_speaker` | Not implemented (audio is a separate effort) |

This ledger covers only the bomb mode's entities; merge it with the generic trigger ledger from master.

## Not implemented

Weapon firing/projectiles, wave-propagation splash (approximated), the 300 ms explosion
animation, random blast points for targetless sites, announcer/sounds/awards, bots, minimap,
decals, dead camera, scoreboard, callvotes, warmup, networking.

## Conflicts / notes for ENGINE_PORT.md

- The brief said master had a trigger/entity system; at the start of this work it did not (it was
  in PR #22). After rebasing onto master the client uses it (`advance_frame_with_entities`), but the
  gametype scans entities itself and does not depend on `simulation::entities`.
- ENGINE_PORT.md says Rhai for built-in gametype scripts: followed. `bevy_ecs` now backs the
  simulation (Option B in `docs/ECS_AND_SCRIPTING.md`): players live in the `Roster` world, map
  entities in `MapEntities`, weapons in `Sim`, HUD values are published per tick. Bomb state itself
  is still an opaque, hashed `ScriptState` map owned by the script (not `Bomb`/`BombSite`
  components), and `GameHost` still drives rules directly rather than through `Sim`.
- Dev tools: `GameHost::reload_script` (hot reload, keeps `ScriptState`, calls `on_reload`) and
  `game::replay` (input-log recording with `state_hash` checkpoints).
- Rules written in a script give up compile-time checking; the headless tests guard them.
- Preserved legacy quirks: `swap_roles` formula; fast-plant tracking omitted.
