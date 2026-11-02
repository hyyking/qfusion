# Client UI design

Covers: bomb loadout selection, team joining, HUD customization, ammo and current-weapon display.
All UI lives in `rust/qf-client`; nothing UI-related enters `qf-engine` or the 125 Hz sim.

## Current state

- `qf-client/src/hud.rs`: pure HUD toolkit (`Screen`, `Anchor`, `Canvas` of coloured `Rect`s, 5x7 font, `HudElement::{update,draw}`); `render.rs` draws the canvas in one 2D overlay pass. Elements: UPS, FPS, REC.
- `bomb.rs`: `BombSession` owns the `GameHost`; `BombHud::from_values(host.hud(id))` parses the script's per-tick HUD map. Keys G/K/T are hard-coded. `host.command(id, "primary" | "secondary" | "drop" ..)` and `host.change_team` already implement loadout and team change.
- `Inventory` (`qf-engine/src/game/inventory.rs`) holds owned weapons, ammo, selected/pending weapon; reachable via `Sim::inventory(slot)`. The client still runs on `ModeSimulation`, so it has no inventory or weapon input until it moves to `Sim`.
- Mouse is grabbed/released in `main.rs` (`mouse_grabbed`, left click / Alt).

## Decisions

1. **UI layer**: keep the custom HUD for in-game elements and add a small immediate-mode menu toolkit (panel, button, list, radio, hit-testing) on top of `Canvas`. No new rendering path or dependency. egui is reserved for developer tooling (issue #9, "not the production HUD/menus"), feature-gated and drawn in a separate pass.
2. **State flow (read-only)**: each frame the client builds a snapshot from the script HUD map (`BombHud`), the local player's `Inventory` (health/armor, owned weapons, ammo, selected and pending weapon, powerups), team and loadout. HUD elements and menus only read it.
3. **Actions**: menus emit `UiAction` values (`Command { name, args }`, `ChangeTeam`, `SetHudLayout`). The client applies them at a tick boundary through `GameHost::command` / `change_team`, the same path keys use today, so UI never touches sim state and determinism is unaffected. Headless mode has no UI.
4. **Input**: `InputContext { Game, Menu }`.
   - Esc toggles the menu. In `Menu`: release the cursor, ignore raw mouse motion, send neutral move/fire commands (the sim keeps ticking), and route click/wheel/keys to the menu.
   - In `Game`: M team menu, B loadout, 1-9 and wheel weapon select (`Sim::push_weapon_input`, once the client is on `Sim`); G/K/T keep working.
   - Team select opens on first join; loadout may open during bomb pre-round.
5. **Screens**
   - Team join: Alpha / Beta / Auto (Spectator: open question) -> `change_team`.
   - Loadout: primary = 2 of RL/LG/EB, secondary = 1 of PG/RG/MG/GL/GB, defaults shown; validated client-side with the script's rules; script rejection is shown. Applied via `primary` / `secondary`.
   - HUD customization: element list, enable toggle, 3x3 anchor presets, margin and scale steppers, live preview, Reset.
6. **HUD elements to add**: `WeaponBar` (owned weapons, selected highlight, per-weapon ammo), `AmmoCounter` (selected weapon, low-ammo colour), `CurrentWeapon` label; move health/armor and the bomb texts out of `BombHud::draw` into elements.
7. **Layout storage**: `HudLayout` = per-element `{ enabled, anchor, margin, scale }` with code defaults, stored as `hud.toml` in the user config dir (`%APPDATA%/qfusion`, `~/.config/qfusion`). Missing or invalid file -> defaults plus a warning; written on Apply. Client-only, never part of sim or demo state.
8. **Determinism**: UI runs at render rate in `qf-client`. Only `UiAction -> command` crosses into the engine, at tick boundaries. Layout and state logic stay pure and unit-tested like `hud.rs`.

## PR split (each branches from `master`)

1. Docs: this file.
2. `HudLayout` model, defaults, load/save; existing UPS/FPS/REC/bomb elements use it (no visual change).
3. Menu framework: `InputContext`, Esc menu, cursor release, widgets, `UiAction` plumbing.
4. Team join screen.
5. Bomb loadout screen.
6. HUD customization screen.
   - 2b. Keybind config (`keybinds.toml`) + action map used by Game and Menu contexts.
7. (a) Move the client onto `Sim` with weapon input (select, fire); (b) weapon bar, ammo, current weapon elements.
8. Optional: bitmap font atlas. Separate: egui debug tools (#9).

PRs 2 and 4 touch `bomb.rs` / `main.rs` HUD drawing and should rebase on the bomb-display fix being investigated separately.

## Resolved decisions

- Menus: custom immediate-mode on the existing HUD.
- Teams: Alpha, Beta, Spectator (spectator = noclip free-fly). No Auto.
- Loadout: selectable any time; applied next round (the script already re-gives in pre-round).
- HUD customization: toggles, anchor/margin/scale, colours, and drag-and-drop (drag edits the anchor + margin, snapping to the nearest anchor).
- Storage: `hud.toml` in the user config dir (adds `serde` + `toml`).
- Keybinds: configurable now, stored in a `binds` section of the config file (`keybinds.toml`), with defaults in code.

## Open item

The ECS migration (PRs 1-10) is merged, but only on the engine side: the client (`main.rs`, `bomb.rs`) still runs `ModeSimulation` and has no `Sim`, inventory or weapon input. The weapon/ammo HUD therefore needs a client-on-`Sim` PR first (PR 7a), then the HUD elements (PR 7b). Spectator noclip must map to a client movement mode.

## Crosshair

The `crosshair` HUD element (`crosshair.rs`) is a centred cross, dot or cross+dot drawn from rectangles, DPI-aware, hidden while a menu is open. Placement, scale, colour and visibility use the normal `[elements.crosshair]` layout entry; the shape is a `[crosshair]` table in `hud.toml` (`style` = `cross`/`dot`/`cross_dot`, `size`, `thickness`, `gap` in logical pixels, clamped). In the HUD editor select XHAIR, then `X` cycles style, `[` `]` size, `,` `.` thickness, `;` `'` gap.
