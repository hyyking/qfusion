//! Weapon firing: input handling, hitscan, projectiles, splash damage and knockback.
//!
//! Everything here is a pure function of its inputs (players, a collision world, the
//! match seed and the tick number), so a match replays identically. The module reports
//! what happened as [`WeaponEvent`]s and never calls the gametype script itself; the
//! host routes `Damage` events through the script's `on_damage` rule hook. Knockback is
//! accumulated in [`Player::impulse`] for whoever owns movement to consume.

use super::combat::Player;
use super::vec::{Bounds, Rng, add, dot, length, scale, sub};
use super::weapons::WeaponId;
use crate::simulation::pmove::{CONTENTS_SOLID, PmoveWorld, Vec3};

pub const EYE_HEIGHT: f32 = 22.0;
pub const PLAYER_MINS: Vec3 = [-16.0, -16.0, -24.0];
pub const PLAYER_MAXS: Vec3 = [16.0, 16.0, 40.0];

/// Legacy `G_KnockBackPush`: `1000 * knockback / mass` with the default mass of 200.
const KNOCKBACK_PUSH: f32 = 1000.0 / 200.0;
const MAX_KNOCKBACK: f32 = 200.0;
const GRENADE_GRAVITY: f32 = 800.0;
const GRENADE_RESTITUTION: f32 = 0.5;
const BULLET_RANGE: f32 = 8192.0;
const PROJECTILE_PRESTEP: f32 = 16.0;
const MAX_PROJECTILE_AGE_MS: u32 = 10_000;
const RNG_PURPOSE_FIRE: u32 = 0x1000;

/// Per-player weapon input, filled from the usercmd buttons each tick.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PlayerInput {
    /// Attack button held (level-triggered).
    pub fire: bool,
    /// Direct weapon selection; consumed by the next step.
    pub select: Option<WeaponId>,
    /// Weapon scroll steps (+1 next, -1 previous); consumed by the next step.
    pub cycle: i32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum WeaponEvent {
    Fired {
        player: usize,
        weapon: WeaponId,
        origin: Vec3,
        direction: Vec3,
    },
    /// Visual for instant weapons: a line from the muzzle to where the shot ended.
    Tracer {
        player: usize,
        weapon: WeaponId,
        from: Vec3,
        to: Vec3,
    },
    Projectile {
        id: u32,
        weapon: WeaponId,
        origin: Vec3,
        velocity: Vec3,
    },
    /// A projectile exploded or a shot hit the world or a player.
    Impact {
        weapon: WeaponId,
        origin: Vec3,
        normal: Vec3,
        victim: Option<usize>,
    },
    Damage {
        victim: usize,
        attacker: usize,
        weapon: WeaponId,
        amount: f32,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Projectile {
    pub id: u32,
    pub owner: usize,
    pub weapon: WeaponId,
    pub origin: Vec3,
    pub velocity: Vec3,
    pub age_ms: u32,
}

#[derive(Clone, Debug, Default)]
pub struct Weapons {
    projectiles: Vec<Projectile>,
    next_id: u32,
}

struct Hit {
    fraction: f32,
    point: Vec3,
    normal: Vec3,
    victim: Option<usize>,
}

fn normalize(vector: Vec3) -> Vec3 {
    let len = length(vector);
    if len < 1e-6 {
        [0.0, 0.0, 1.0]
    } else {
        scale(vector, 1.0 / len)
    }
}

fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn lerp(a: Vec3, b: Vec3, t: f32) -> Vec3 {
    add(a, scale(sub(b, a), t))
}

/// Entry fraction of the segment into the box, `None` if it misses.
fn segment_box(start: Vec3, end: Vec3, bounds: &Bounds) -> Option<f32> {
    let (mut near, mut far) = (0.0_f32, 1.0_f32);
    for axis in 0..3 {
        let delta = end[axis] - start[axis];
        if delta.abs() < 1e-9 {
            if start[axis] < bounds.mins[axis] || start[axis] > bounds.maxs[axis] {
                return None;
            }
            continue;
        }
        let mut t0 = (bounds.mins[axis] - start[axis]) / delta;
        let mut t1 = (bounds.maxs[axis] - start[axis]) / delta;
        if t0 > t1 {
            std::mem::swap(&mut t0, &mut t1);
        }
        near = near.max(t0);
        far = far.min(t1);
        if near > far {
            return None;
        }
    }
    Some(near)
}

fn sweep(players: &[Player], world: &dyn PmoveWorld, ignore: usize, start: Vec3, end: Vec3) -> Hit {
    let trace = world.trace(start, [0.0; 3], [0.0; 3], end, CONTENTS_SOLID);
    let mut hit = Hit {
        fraction: trace.fraction,
        point: if trace.fraction >= 1.0 {
            end
        } else {
            trace.end_position
        },
        normal: trace.plane_normal,
        victim: None,
    };
    for (index, player) in players.iter().enumerate() {
        if !player.alive || index == ignore {
            continue;
        }
        let bounds = Bounds::at(player.origin, PLAYER_MINS, PLAYER_MAXS);
        if let Some(fraction) = segment_box(start, end, &bounds)
            && fraction < hit.fraction
        {
            hit = Hit {
                fraction,
                point: lerp(start, end, fraction),
                normal: normalize(sub(start, end)),
                victim: Some(index),
            };
        }
    }
    hit
}

fn push(player: &mut Player, direction: Vec3, knockback: f32) {
    let knockback = knockback.min(MAX_KNOCKBACK);
    if knockback > 0.0 {
        player.impulse = add(player.impulse, scale(direction, knockback * KNOCKBACK_PUSH));
    }
}

fn signed(rng: &mut Rng) -> f32 {
    (rng.below(2001) as f32 - 1000.0) / 1000.0
}

fn muzzle(player: &Player) -> Vec3 {
    add(player.origin, [0.0, 0.0, EYE_HEIGHT])
}

/// Radius damage around `center` with linear falloff to the nearest point of each
/// player's box; the player hit directly (`skip`) is handled by the caller.
fn splash(
    players: &mut [Player],
    world: &dyn PmoveWorld,
    weapon: WeaponId,
    owner: usize,
    center: Vec3,
    skip: Option<usize>,
    events: &mut Vec<WeaponEvent>,
) {
    let def = weapon.def();
    if def.splash_radius <= 0.0 {
        return;
    }
    for (index, player) in players.iter_mut().enumerate() {
        if !player.alive || Some(index) == skip {
            continue;
        }
        let bounds = Bounds::at(player.origin, PLAYER_MINS, PLAYER_MAXS);
        let nearest =
            [0, 1, 2].map(|axis| center[axis].clamp(bounds.mins[axis], bounds.maxs[axis]));
        let distance = length(sub(nearest, center));
        if distance >= def.splash_radius {
            continue;
        }
        if world
            .trace(center, [0.0; 3], [0.0; 3], bounds.center(), CONTENTS_SOLID)
            .fraction
            < 1.0
        {
            continue;
        }
        let fraction = 1.0 - distance / def.splash_radius;
        let mut damage = (def.damage * fraction).max(def.splash_min_damage);
        let knockback = (def.knockback * fraction).max(def.splash_min_knockback);
        if index == owner {
            damage *= def.self_damage_ratio;
        }
        push(player, normalize(sub(bounds.center(), center)), knockback);
        if damage > 0.0 {
            events.push(WeaponEvent::Damage {
                victim: index,
                attacker: owner,
                weapon,
                amount: damage,
            });
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn detonate(
    players: &mut [Player],
    world: &dyn PmoveWorld,
    weapon: WeaponId,
    owner: usize,
    point: Vec3,
    normal: Vec3,
    direction: Vec3,
    victim: Option<usize>,
    events: &mut Vec<WeaponEvent>,
) {
    let def = weapon.def();
    if let Some(index) = victim {
        push(&mut players[index], direction, def.knockback);
        events.push(WeaponEvent::Damage {
            victim: index,
            attacker: owner,
            weapon,
            amount: def.damage,
        });
    }
    events.push(WeaponEvent::Impact {
        weapon,
        origin: point,
        normal,
        victim,
    });
    // Back the blast off the surface so it is not centered inside the wall.
    let center = add(point, scale(normal, 1.0));
    splash(players, world, weapon, owner, center, victim, events);
}

impl Weapons {
    pub fn projectiles(&self) -> &[Projectile] {
        &self.projectiles
    }

    /// Advances every player's weapon and all projectiles by one `dt_ms` tick.
    pub fn step(
        &mut self,
        players: &mut [Player],
        world: &dyn PmoveWorld,
        seed: u64,
        tick: u64,
        dt_ms: u32,
    ) -> Vec<WeaponEvent> {
        let mut events = Vec::new();
        for index in 0..players.len() {
            self.step_player(players, world, index, seed, tick, dt_ms, &mut events);
        }
        self.step_projectiles(players, world, dt_ms, &mut events);
        events
    }

    #[allow(clippy::too_many_arguments)]
    fn step_player(
        &mut self,
        players: &mut [Player],
        world: &dyn PmoveWorld,
        index: usize,
        seed: u64,
        tick: u64,
        dt_ms: u32,
        events: &mut Vec<WeaponEvent>,
    ) {
        let player = &mut players[index];
        let input = std::mem::take(&mut player.input);
        player.input.fire = input.fire;
        if !player.alive {
            return;
        }
        player.inventory.tick(dt_ms);
        if let Some(weapon) = input.select {
            player.inventory.request_select(weapon);
        }
        if input.cycle != 0
            && let Some(weapon) = player.inventory.cycle(input.cycle)
        {
            player.inventory.request_select(weapon);
        }
        if !input.fire {
            return;
        }
        let Some(weapon) = player.inventory.selected() else {
            return;
        };
        if player.inventory.state() == super::inventory::WeaponState::Ready
            && !player.inventory.has_ammo(weapon)
        {
            player.inventory.select_best();
            return;
        }
        if !player.inventory.can_fire() {
            return;
        }
        player.inventory.spend_shot(weapon);
        player.inventory.begin_refire();
        let mut rng = Rng::substream(seed, tick, RNG_PURPOSE_FIRE + index as u32);
        self.fire(players, world, index, weapon, &mut rng, events);
    }

    fn fire(
        &mut self,
        players: &mut [Player],
        world: &dyn PmoveWorld,
        index: usize,
        weapon: WeaponId,
        rng: &mut Rng,
        events: &mut Vec<WeaponEvent>,
    ) {
        let def = weapon.def();
        let start = muzzle(&players[index]);
        let forward = players[index].forward();
        events.push(WeaponEvent::Fired {
            player: index,
            weapon,
            origin: start,
            direction: forward,
        });
        if def.is_projectile() {
            let spawn = world.trace(
                start,
                [0.0; 3],
                [0.0; 3],
                add(start, scale(forward, PROJECTILE_PRESTEP)),
                CONTENTS_SOLID,
            );
            let origin = if spawn.fraction >= 1.0 {
                add(start, scale(forward, PROJECTILE_PRESTEP))
            } else {
                spawn.end_position
            };
            let velocity = scale(forward, def.speed);
            let id = self.next_id;
            self.next_id += 1;
            self.projectiles.push(Projectile {
                id,
                owner: index,
                weapon,
                origin,
                velocity,
                age_ms: 0,
            });
            events.push(WeaponEvent::Projectile {
                id,
                weapon,
                origin,
                velocity,
            });
            return;
        }
        let right = {
            let right = cross(forward, [0.0, 0.0, 1.0]);
            if length(right) < 1e-3 {
                [0.0, 1.0, 0.0]
            } else {
                normalize(right)
            }
        };
        let up = cross(right, forward);
        let range = if def.timeout > 0 {
            def.timeout as f32
        } else {
            BULLET_RANGE
        };
        for _ in 0..def.projectiles_per_shot.max(1) {
            let (r, u) = (signed(rng) * def.spread, signed(rng) * def.v_spread);
            let end = add(
                add(start, scale(forward, range)),
                add(scale(right, r), scale(up, u)),
            );
            let hit = sweep(players, world, index, start, end);
            events.push(WeaponEvent::Tracer {
                player: index,
                weapon,
                from: start,
                to: hit.point,
            });
            if let Some(victim) = hit.victim {
                push(
                    &mut players[victim],
                    normalize(sub(end, start)),
                    def.knockback,
                );
                events.push(WeaponEvent::Damage {
                    victim,
                    attacker: index,
                    weapon,
                    amount: def.damage,
                });
            }
            if hit.fraction < 1.0 {
                events.push(WeaponEvent::Impact {
                    weapon,
                    origin: hit.point,
                    normal: hit.normal,
                    victim: hit.victim,
                });
            }
        }
    }

    fn step_projectiles(
        &mut self,
        players: &mut [Player],
        world: &dyn PmoveWorld,
        dt_ms: u32,
        events: &mut Vec<WeaponEvent>,
    ) {
        let dt = dt_ms as f32 / 1000.0;
        let mut alive = Vec::with_capacity(self.projectiles.len());
        for mut projectile in std::mem::take(&mut self.projectiles) {
            let def = projectile.weapon.def();
            let grenade = projectile.weapon == WeaponId::GrenadeLauncher;
            projectile.age_ms += dt_ms;
            if grenade {
                projectile.velocity[2] -= GRENADE_GRAVITY * dt;
            }
            let end = add(projectile.origin, scale(projectile.velocity, dt));
            let hit = sweep(players, world, projectile.owner, projectile.origin, end);
            let direction = normalize(projectile.velocity);
            if hit.fraction < 1.0 {
                if hit.victim.is_none() && grenade {
                    let normal = hit.normal;
                    let along = dot(projectile.velocity, normal);
                    projectile.velocity = sub(
                        projectile.velocity,
                        scale(normal, (1.0 + GRENADE_RESTITUTION) * along),
                    );
                    projectile.origin = add(hit.point, scale(normal, 0.5));
                } else {
                    detonate(
                        players,
                        world,
                        projectile.weapon,
                        projectile.owner,
                        hit.point,
                        hit.normal,
                        direction,
                        hit.victim,
                        events,
                    );
                    continue;
                }
            } else {
                projectile.origin = end;
            }
            let lifetime = if def.timeout > 0 {
                def.timeout
            } else {
                MAX_PROJECTILE_AGE_MS
            };
            if projectile.age_ms >= lifetime.min(MAX_PROJECTILE_AGE_MS) {
                detonate(
                    players,
                    world,
                    projectile.weapon,
                    projectile.owner,
                    projectile.origin,
                    [0.0, 0.0, 1.0],
                    direction,
                    None,
                    events,
                );
                continue;
            }
            alive.push(projectile);
        }
        self.projectiles = alive;
    }
}

#[cfg(test)]
mod tests {
    use super::super::combat::Team;
    use super::*;
    use crate::simulation::pmove::PmTrace;
    use WeaponId::*;

    /// A solid wall on the plane x = 1000 facing -x.
    struct Wall;

    impl PmoveWorld for Wall {
        fn trace(&self, start: Vec3, _: Vec3, _: Vec3, end: Vec3, _: u32) -> PmTrace {
            if start[0] < 1000.0 && end[0] >= 1000.0 {
                let fraction = (1000.0 - start[0]) / (end[0] - start[0]);
                let mut trace = PmTrace::miss(end);
                trace.fraction = fraction;
                trace.end_position = lerp(start, end, fraction);
                trace.plane_normal = [-1.0, 0.0, 0.0];
                return trace;
            }
            PmTrace::miss(end)
        }

        fn point_contents(&self, _: Vec3) -> u32 {
            0
        }
    }

    fn player(origin: Vec3, weapon: WeaponId, ammo: u32) -> Player {
        let mut player = Player::new(Team::Alpha);
        player.alive = true;
        player.health = 100.0;
        player.origin = origin;
        player.inventory.give_weapon(weapon, ammo);
        player
    }

    fn settle(weapons: &mut Weapons, players: &mut [Player]) {
        for tick in 0..10 {
            weapons.step(players, &Wall, 1, tick, 8);
        }
    }

    fn damage(events: &[WeaponEvent]) -> Vec<(usize, f32)> {
        events
            .iter()
            .filter_map(|event| match event {
                WeaponEvent::Damage { victim, amount, .. } => Some((*victim, *amount)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn electrobolt_hits_the_player_in_front_and_pushes_it() {
        let mut players = [
            player([0.0; 3], Electrobolt, 10),
            player([300.0, 0.0, 0.0], Machinegun, 0),
        ];
        let mut weapons = Weapons::default();
        settle(&mut weapons, &mut players);
        players[0].input.fire = true;
        let events = weapons.step(&mut players, &Wall, 1, 11, 8);
        assert_eq!(damage(&events), vec![(1, 70.0)]);
        assert!(players[1].impulse[0] > 0.0);
        assert_eq!(players[0].inventory.ammo(Electrobolt), 9);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, WeaponEvent::Tracer { .. }))
        );
    }

    #[test]
    fn hitscan_stops_at_the_wall() {
        let mut players = [player([500.0, 0.0, 0.0], Electrobolt, 10)];
        let mut weapons = Weapons::default();
        settle(&mut weapons, &mut players);
        players[0].input.fire = true;
        let events = weapons.step(&mut players, &Wall, 1, 11, 8);
        let to = events
            .iter()
            .find_map(|e| match e {
                WeaponEvent::Tracer { to, .. } => Some(*to),
                _ => None,
            })
            .unwrap();
        assert!((to[0] - 1000.0).abs() < 1e-2, "{to:?}");
        assert!(
            events
                .iter()
                .any(|e| matches!(e, WeaponEvent::Impact { victim: None, .. }))
        );
    }

    #[test]
    fn refire_limits_the_rate_and_ammo_is_spent() {
        let mut players = [player([0.0; 3], Machinegun, 100)];
        let mut weapons = Weapons::default();
        settle(&mut weapons, &mut players);
        let mut shots = 0;
        for tick in 0..125 {
            players[0].input.fire = true;
            let events = weapons.step(&mut players, &Wall, 1, 100 + tick, 8);
            shots += events
                .iter()
                .filter(|e| matches!(e, WeaponEvent::Fired { .. }))
                .count();
        }
        // 1 second at a 50 ms refire on an 8 ms grid fires every 7th tick.
        assert!((17..=21).contains(&shots), "{shots}");
        assert_eq!(players[0].inventory.ammo(Machinegun), 100 - shots as u32);
    }

    #[test]
    fn rocket_flies_hits_the_wall_and_splashes_the_shooter() {
        let mut players = [player([990.0 - 40.0, 0.0, 0.0], RocketLauncher, 5)];
        let mut weapons = Weapons::default();
        settle(&mut weapons, &mut players);
        players[0].input.fire = true;
        let mut all = Vec::new();
        for tick in 0..40 {
            all.extend(weapons.step(&mut players, &Wall, 1, 20 + tick, 8));
        }
        assert!(
            all.iter()
                .any(|e| matches!(e, WeaponEvent::Projectile { .. }))
        );
        assert!(
            all.iter()
                .any(|e| matches!(e, WeaponEvent::Impact { victim: None, .. }))
        );
        assert!(weapons.projectiles().is_empty());
        assert!(
            damage(&all)
                .iter()
                .any(|&(victim, amount)| victim == 0 && amount > 0.0)
        );
        assert!(
            players[0].impulse[0] < 0.0,
            "rocket jump pushes away from the wall"
        );
    }

    #[test]
    fn rocket_direct_hit_does_full_damage_without_splashing_twice() {
        let mut players = [
            player([0.0; 3], RocketLauncher, 5),
            player([200.0, 0.0, 0.0], Machinegun, 0),
        ];
        let mut weapons = Weapons::default();
        settle(&mut weapons, &mut players);
        players[0].input.fire = true;
        let mut all = Vec::new();
        for tick in 0..30 {
            all.extend(weapons.step(&mut players, &Wall, 1, 20 + tick, 8));
            players[0].input.fire = false;
        }
        let hits: Vec<_> = damage(&all).into_iter().filter(|d| d.0 == 1).collect();
        assert_eq!(hits, vec![(1, 90.0)]);
    }

    #[test]
    fn splash_falls_off_with_distance() {
        let mut players = [
            player([0.0, 0.0, 0.0], Machinegun, 0),
            player([60.0, 0.0, 0.0], Machinegun, 0),
        ];
        let mut events = Vec::new();
        splash(
            &mut players,
            &Wall,
            RocketLauncher,
            0,
            [-30.0, 0.0, 0.0],
            None,
            &mut events,
        );
        let hurt = damage(&events);
        let own = hurt.iter().find(|d| d.0 == 0).unwrap().1;
        let other = hurt.iter().find(|d| d.0 == 1).unwrap().1;
        assert!(own < RocketLauncher.def().damage);
        assert!(other > 0.0 && other < 90.0);
    }

    #[test]
    fn empty_weapon_switches_to_the_best_with_ammo() {
        let mut players = [player([0.0; 3], RocketLauncher, 0)];
        players[0].inventory.give_weapon(Machinegun, 50);
        let mut weapons = Weapons::default();
        settle(&mut weapons, &mut players);
        players[0].input.fire = true;
        weapons.step(&mut players, &Wall, 1, 11, 8);
        assert_eq!(players[0].inventory.target(), Some(Machinegun));
    }

    #[test]
    fn select_and_cycle_inputs_change_weapons_once() {
        let mut players = [player([0.0; 3], RocketLauncher, 5)];
        players[0].inventory.give_weapon(Plasmagun, 50);
        let mut weapons = Weapons::default();
        settle(&mut weapons, &mut players);
        players[0].input.select = Some(Plasmagun);
        weapons.step(&mut players, &Wall, 1, 11, 8);
        assert_eq!(players[0].inventory.target(), Some(Plasmagun));
        assert_eq!(players[0].input.select, None);
    }

    #[test]
    fn grenade_bounces_then_explodes_on_timeout() {
        let mut players = [player([0.0; 3], GrenadeLauncher, 5)];
        players[0].pitch = -45.0;
        let mut weapons = Weapons::default();
        settle(&mut weapons, &mut players);
        players[0].input.fire = true;
        let mut impact = None;
        for tick in 0..250 {
            let events = weapons.step(&mut players, &Wall, 1, 20 + tick, 8);
            players[0].input.fire = false;
            if let Some(WeaponEvent::Impact { .. }) = events
                .iter()
                .find(|e| matches!(e, WeaponEvent::Impact { .. }))
            {
                impact = Some(tick);
                break;
            }
        }
        let tick = impact.expect("grenade explodes");
        assert!((150..=160).contains(&tick), "{tick}");
    }

    #[test]
    fn same_seed_replays_identically() {
        let run = || {
            let mut players = [
                player([0.0; 3], Riotgun, 10),
                player([400.0, 10.0, 0.0], Machinegun, 0),
            ];
            let mut weapons = Weapons::default();
            settle(&mut weapons, &mut players);
            players[0].input.fire = true;
            weapons.step(&mut players, &Wall, 7, 50, 8)
        };
        assert_eq!(run(), run());
    }
}
