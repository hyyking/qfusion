//! Generic, map-agnostic host for Rhai gametype scripts.
//!
//! The script owns the rules (round lifecycle, objectives, scoring, HUD data). The host
//! only provides primitives: map entities, collision traces, player state with
//! health/armor, deterministic randomness, a fixed 125 Hz clock and an event queue.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};

use rhai::{
    AST, CallFnOptions, Dynamic, Engine, EvalAltResult, FuncArgs, ImmutableString, Map, Scope,
};

use crate::content::bsp::entities::Entity;
use crate::content::bsp::{Bsp, Model};
use crate::simulation::bsp_world::BspWorld;
use crate::simulation::entities::{BrushModels, Gametype, MapEntities, MoverWorld, TriggerEvent};
use crate::simulation::pmove::{PmTrace, PmoveWorld, Vec3};

use super::combat::{Player, Team};
use super::roster::Roster;
use super::vec::{Bounds, Rng, add, distance, dot, length, scale, sub};

pub const TICK_MS: u64 = 8;
type ScriptResult<T> = Result<T, Box<EvalAltResult>>;

pub trait GameWorld: PmoveWorld + Send + Sync {}
impl<T: PmoveWorld + Send + Sync> GameWorld for T {}

/// Brush-model queries shared with the engine's entity world.
pub trait GameModels: BrushModels + Send + Sync {}
impl<T: BrushModels + Send + Sync> GameModels for T {}

/// Inline models known only by their bounds (used when no parsed BSP is available).
struct ModelBounds(Vec<(Vec3, Vec3)>);

impl BrushModels for ModelBounds {
    fn model_bounds(&self, index: usize) -> Option<(Vec3, Vec3)> {
        self.0.get(index).copied()
    }

    fn box_touches_model(&self, index: usize, origin: Vec3, mins: Vec3, maxs: Vec3) -> bool {
        self.0.get(index).is_some_and(|(lo, hi)| {
            (0..3).all(|axis| {
                maxs[axis] > lo[axis] + origin[axis] && mins[axis] < hi[axis] + origin[axis]
            })
        })
    }
}

/// Owns a parsed BSP so scripts can trace without borrowing it.
pub struct OwnedBspWorld(pub Arc<Bsp>);

impl PmoveWorld for OwnedBspWorld {
    fn trace(&self, start: Vec3, mins: Vec3, maxs: Vec3, end: Vec3, mask: u32) -> PmTrace {
        BspWorld::new(&self.0).trace(start, mins, maxs, end, mask)
    }

    fn point_contents(&self, point: Vec3) -> u32 {
        BspWorld::new(&self.0).point_contents(point)
    }
}

#[derive(Debug)]
pub enum GameError {
    Script(String),
}

impl fmt::Display for GameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self::Script(message) = self;
        write!(f, "gametype script error: {message}")
    }
}

impl std::error::Error for GameError {}

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
    Vec3(Vec3),
}

impl Value {
    fn from_dynamic(value: &Dynamic) -> Option<Self> {
        if let Ok(int) = value.as_int() {
            Some(Self::Int(int))
        } else if let Ok(float) = value.as_float() {
            Some(Self::Float(float))
        } else if let Ok(flag) = value.as_bool() {
            Some(Self::Bool(flag))
        } else if let Some(text) = value.clone().try_cast::<ImmutableString>() {
            Some(Self::Str(text.to_string()))
        } else {
            value
                .clone()
                .try_cast::<rhai::Array>()
                .and_then(|array| to_vec3(&array).ok())
                .map(Self::Vec3)
        }
    }
}

/// Something the script reported to the outside world (client, tests, server).
#[derive(Clone, Debug, PartialEq)]
pub struct GameEvent {
    pub kind: String,
    pub fields: BTreeMap<String, Value>,
}

impl GameEvent {
    pub fn int(&self, key: &str) -> Option<i64> {
        match self.fields.get(key)? {
            Value::Int(value) => Some(*value),
            _ => None,
        }
    }

    pub fn text(&self, key: &str) -> Option<&str> {
        match self.fields.get(key)? {
            Value::Str(value) => Some(value),
            _ => None,
        }
    }

    pub fn float(&self, key: &str) -> Option<f64> {
        match self.fields.get(key)? {
            Value::Float(value) => Some(*value),
            Value::Int(value) => Some(*value as f64),
            _ => None,
        }
    }

    pub fn vec3(&self, key: &str) -> Option<Vec3> {
        match self.fields.get(key)? {
            Value::Vec3(value) => Some(*value),
            _ => None,
        }
    }
}

struct Core {
    now: u64,
    roster: Roster,
    map: MapEntities,
    models: Arc<dyn GameModels>,
    entities: rhai::Array,
    world: Arc<dyn GameWorld>,
    world_bounds: Bounds,
    rng: Rng,
    events: Vec<GameEvent>,
}

impl Core {
    /// Static collision plus the map's brush movers at their current pose.
    fn trace(&self, start: Vec3, mins: Vec3, maxs: Vec3, end: Vec3) -> PmTrace {
        MoverWorld::new(&*self.world, &*self.models, self.map.mover_poses()).trace(
            start,
            mins,
            maxs,
            end,
            crate::simulation::pmove::CONTENTS_SOLID,
        )
    }

    /// Runs the map's due thinks and forwards use-chain events to the script queue.
    fn run_map(&mut self) {
        self.map.think(self.now as i64);
        self.forward_map_events();
    }

    fn forward_map_events(&mut self) {
        for event in self.map.drain_events() {
            let (kind, fields) = match event {
                TriggerEvent::Used { source, target } => (
                    "use",
                    [("source", source), ("target", target)]
                        .map(|(k, v)| (k.to_owned(), Value::Int(v as i64))),
                ),
                _ => continue,
            };
            self.events.push(GameEvent {
                kind: kind.into(),
                fields: fields.into_iter().collect(),
            });
        }
    }
}

#[derive(Clone)]
pub struct Game(Arc<Mutex<Core>>);

impl Game {
    fn core(&self) -> MutexGuard<'_, Core> {
        self.0.lock().expect("game state poisoned")
    }

    fn player_mut<R>(&self, id: i64, f: impl FnOnce(&mut Player) -> R) -> ScriptResult<R> {
        let mut core = self.core();
        let player = usize::try_from(id)
            .ok()
            .and_then(|id| core.roster.get_mut(id))
            .ok_or_else(|| format!("no such player {id}"))?;
        Ok(f(player))
    }
}

fn entity_index(index: i64) -> ScriptResult<usize> {
    usize::try_from(index).map_err(|_| format!("no such entity {index}").into())
}

fn number(value: &Dynamic) -> ScriptResult<f32> {
    value
        .as_float()
        .map(|f| f as f32)
        .or_else(|_| value.as_int().map(|i| i as f32))
        .map_err(|_| "expected a number".into())
}

fn to_vec3(array: &rhai::Array) -> ScriptResult<Vec3> {
    if array.len() != 3 {
        return Err("expected a 3-component vector".into());
    }
    Ok([number(&array[0])?, number(&array[1])?, number(&array[2])?])
}

fn from_vec3(vector: Vec3) -> rhai::Array {
    vector
        .iter()
        .map(|c| Dynamic::from_float(f64::from(*c)))
        .collect()
}

fn parse_vec3(text: &str) -> Option<Vec3> {
    let parts: Vec<f32> = text
        .split_ascii_whitespace()
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()?;
    let vector: Vec3 = parts.try_into().ok()?;
    vector.iter().all(|c| c.is_finite()).then_some(vector)
}

fn export_entity(entity: &Entity, models: &[Model]) -> Map {
    let mut map = Map::new();
    for (key, value) in &entity.properties {
        let parsed = match key.as_str() {
            "origin" | "angles" => parse_vec3(value).map(|v| Dynamic::from(from_vec3(v))),
            _ => None,
        };
        map.insert(
            key.as_str().into(),
            parsed.unwrap_or_else(|| Dynamic::from(value.clone())),
        );
    }
    let model = entity
        .get("model")
        .and_then(|model| model.strip_prefix('*'))
        .and_then(|index| index.parse::<usize>().ok())
        .and_then(|index| models.get(index));
    if let Some(model) = model {
        let mut bounds = Map::new();
        bounds.insert("mins".into(), Dynamic::from(from_vec3(model.mins)));
        bounds.insert("maxs".into(), Dynamic::from(from_vec3(model.maxs)));
        map.insert("bounds".into(), Dynamic::from(bounds));
    }
    map
}

fn register_api(engine: &mut Engine) {
    engine.register_type_with_name::<Game>("Game");
    engine.register_fn("now", |g: &mut Game| g.core().now as i64);
    engine.register_fn("rand", |g: &mut Game, count: i64| -> ScriptResult<i64> {
        if count <= 0 {
            return Err("rand needs a positive count".into());
        }
        Ok(g.core().rng.below(count as usize) as i64)
    });
    engine.register_fn("entities", |g: &mut Game| g.core().entities.clone());
    engine.register_fn(
        "trace",
        |g: &mut Game,
         start: rhai::Array,
         end: rhai::Array,
         mins: rhai::Array,
         maxs: rhai::Array|
         -> ScriptResult<Map> {
            let trace = g.core().trace(
                to_vec3(&start)?,
                to_vec3(&mins)?,
                to_vec3(&maxs)?,
                to_vec3(&end)?,
            );
            let mut map = Map::new();
            map.insert(
                "fraction".into(),
                Dynamic::from_float(f64::from(trace.fraction)),
            );
            map.insert("end".into(), Dynamic::from(from_vec3(trace.end_position)));
            map.insert(
                "normal".into(),
                Dynamic::from(from_vec3(trace.plane_normal)),
            );
            map.insert("start_solid".into(), Dynamic::from(trace.start_solid));
            Ok(map)
        },
    );
    engine.register_fn(
        "los",
        |g: &mut Game, from: rhai::Array, to: rhai::Array| -> ScriptResult<bool> {
            let trace = g
                .core()
                .trace(to_vec3(&from)?, [0.0; 3], [0.0; 3], to_vec3(&to)?);
            Ok(trace.fraction >= 1.0)
        },
    );
    engine.register_fn(
        "use_entity",
        |g: &mut Game, index: i64| -> ScriptResult<bool> {
            let mut core = g.core();
            let (index, now) = (entity_index(index)?, core.now as i64);
            let fired = core.map.activate(index, now);
            core.forward_map_events();
            Ok(fired)
        },
    );
    engine.register_fn(
        "fire_targets",
        |g: &mut Game, index: i64| -> ScriptResult<bool> {
            let mut core = g.core();
            let (index, now) = (entity_index(index)?, core.now as i64);
            let fired = core.map.use_targets_of(index, now);
            core.forward_map_events();
            Ok(fired)
        },
    );
    engine.register_fn(
        "in_world",
        |g: &mut Game, point: rhai::Array| -> ScriptResult<bool> {
            Ok(g.core().world_bounds.contains(to_vec3(&point)?))
        },
    );
    engine.register_fn("player_count", |g: &mut Game| g.core().roster.len() as i64);
    engine.register_fn("team", |g: &mut Game, id: i64| {
        g.player_mut(id, |p| p.team.index() as i64)
    });
    engine.register_fn("alive", |g: &mut Game, id: i64| {
        g.player_mut(id, |p| p.alive)
    });
    engine.register_fn("health", |g: &mut Game, id: i64| {
        g.player_mut(id, |p| f64::from(p.health))
    });
    engine.register_fn("armor", |g: &mut Game, id: i64| {
        g.player_mut(id, |p| f64::from(p.armor))
    });
    engine.register_fn("origin", |g: &mut Game, id: i64| {
        g.player_mut(id, |p| from_vec3(p.origin))
    });
    engine.register_fn("velocity", |g: &mut Game, id: i64| {
        g.player_mut(id, |p| from_vec3(p.velocity))
    });
    engine.register_fn("forward", |g: &mut Game, id: i64| {
        g.player_mut(id, |p| from_vec3(p.forward()))
    });
    engine.register_fn(
        "set_team",
        |g: &mut Game, id: i64, team: i64| -> ScriptResult<()> {
            let team = Team::from_index(team).ok_or("team must be 0 or 1")?;
            g.player_mut(id, |p| p.team = team)
        },
    );
    engine.register_fn(
        "spawn_at",
        |g: &mut Game, id: i64, origin: rhai::Array, yaw: Dynamic| -> ScriptResult<()> {
            let origin = to_vec3(&origin)?;
            let yaw = number(&yaw)?;
            g.player_mut(id, |p| {
                p.alive = true;
                p.give_inventory();
                p.origin = origin;
                p.velocity = [0.0; 3];
                p.yaw = yaw;
                p.pitch = 0.0;
            })?;
            let mut fields = BTreeMap::new();
            fields.insert("player".into(), Value::Int(id));
            fields.insert("origin".into(), Value::Vec3(origin));
            fields.insert("yaw".into(), Value::Float(f64::from(yaw)));
            g.core().events.push(GameEvent {
                kind: "spawn".into(),
                fields,
            });
            Ok(())
        },
    );
    engine.register_fn("ghost", |g: &mut Game, id: i64| {
        g.player_mut(id, |p| {
            p.alive = false;
            p.health = 0.0;
        })
    });
    engine.register_fn(
        "apply_damage",
        |g: &mut Game, id: i64, amount: Dynamic| -> ScriptResult<bool> {
            let amount = number(&amount)?;
            g.player_mut(id, |p| p.take_damage(amount))
        },
    );
    engine.register_fn(
        "set_health",
        |g: &mut Game, id: i64, value: Dynamic| -> ScriptResult<()> {
            let value = number(&value)?;
            g.player_mut(id, |p| p.health = value)
        },
    );
    engine.register_fn(
        "set_armor",
        |g: &mut Game, id: i64, value: Dynamic| -> ScriptResult<()> {
            let value = number(&value)?;
            g.player_mut(id, |p| p.armor = value)
        },
    );
    engine.register_fn("stat", |g: &mut Game, id: i64, name: &str| {
        g.player_mut(id, |p| p.stats.get(name).copied().unwrap_or(0))
    });
    engine.register_fn(
        "set_stat",
        |g: &mut Game, id: i64, name: &str, value: i64| {
            g.player_mut(id, |p| {
                p.stats.insert(name.to_owned(), value);
            })
        },
    );
    engine.register_fn("clear_weapons", |g: &mut Game, id: i64| {
        g.player_mut(id, |p| p.weapons.clear())
    });
    engine.register_fn(
        "give_weapon",
        |g: &mut Game, id: i64, name: &str, ammo: i64| {
            g.player_mut(id, |p| {
                p.weapons.push((name.to_owned(), ammo.max(0) as u32))
            })
        },
    );
    engine.register_fn("set_movement_locked", |g: &mut Game, locked: bool| {
        g.core().roster.set_movement_locked(locked);
    });
    engine.register_fn("emit", |g: &mut Game, kind: &str, data: Map| {
        let fields = data
            .iter()
            .filter_map(|(key, value)| Some((key.to_string(), Value::from_dynamic(value)?)))
            .collect();
        g.core().events.push(GameEvent {
            kind: kind.to_owned(),
            fields,
        });
    });
    engine.register_fn(
        "to_num",
        |text: &str, default: Dynamic| -> ScriptResult<f64> {
            let default = number(&default)?;
            Ok(text
                .trim()
                .parse::<f64>()
                .ok()
                .filter(|v| v.is_finite())
                .unwrap_or(f64::from(default)))
        },
    );
    engine.register_fn(
        "vadd",
        |a: rhai::Array, b: rhai::Array| -> ScriptResult<rhai::Array> {
            Ok(from_vec3(add(to_vec3(&a)?, to_vec3(&b)?)))
        },
    );
    engine.register_fn(
        "vsub",
        |a: rhai::Array, b: rhai::Array| -> ScriptResult<rhai::Array> {
            Ok(from_vec3(sub(to_vec3(&a)?, to_vec3(&b)?)))
        },
    );
    engine.register_fn(
        "vscale",
        |a: rhai::Array, s: Dynamic| -> ScriptResult<rhai::Array> {
            Ok(from_vec3(scale(to_vec3(&a)?, number(&s)?)))
        },
    );
    engine.register_fn(
        "vdot",
        |a: rhai::Array, b: rhai::Array| -> ScriptResult<f64> {
            Ok(f64::from(dot(to_vec3(&a)?, to_vec3(&b)?)))
        },
    );
    engine.register_fn("vlen", |a: rhai::Array| -> ScriptResult<f64> {
        Ok(f64::from(length(to_vec3(&a)?)))
    });
    engine.register_fn(
        "vdist",
        |a: rhai::Array, b: rhai::Array| -> ScriptResult<f64> {
            Ok(f64::from(distance(to_vec3(&a)?, to_vec3(&b)?)))
        },
    );
}

/// A loaded gametype running against one map.
pub struct GameHost {
    game: Game,
    engine: Engine,
    ast: AST,
    hooks: BTreeSet<String>,
    state: Dynamic,
    faults: u32,
}

impl fmt::Debug for GameHost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("GameHost")
    }
}

fn script_error(error: impl fmt::Display) -> GameError {
    GameError::Script(error.to_string())
}

/// Operation budget for one hook invocation. Counted in script operations, never wall
/// time, so overruns abort at the same point on every machine.
pub const MAX_HOOK_OPERATIONS: u64 = 2_000_000;

/// A raw engine with only deterministic packages: no `timestamp`/`Instant` (wall clock),
/// no `eval`, no module resolution, and hard limits on work and data size.
fn sandboxed_engine() -> Engine {
    use rhai::packages::{
        ArithmeticPackage, BasicArrayPackage, BasicFnPackage, BasicIteratorPackage,
        BasicMapPackage, BasicMathPackage, BasicStringPackage, CorePackage, LogicPackage,
        MoreStringPackage, Package,
    };
    let mut engine = Engine::new_raw();
    CorePackage::new().register_into_engine(&mut engine);
    ArithmeticPackage::new().register_into_engine(&mut engine);
    LogicPackage::new().register_into_engine(&mut engine);
    BasicStringPackage::new().register_into_engine(&mut engine);
    MoreStringPackage::new().register_into_engine(&mut engine);
    BasicMathPackage::new().register_into_engine(&mut engine);
    BasicArrayPackage::new().register_into_engine(&mut engine);
    BasicMapPackage::new().register_into_engine(&mut engine);
    BasicIteratorPackage::new().register_into_engine(&mut engine);
    BasicFnPackage::new().register_into_engine(&mut engine);
    engine.disable_symbol("eval");
    engine.set_max_expr_depths(128, 64);
    engine.set_max_call_levels(32);
    engine.set_max_operations(MAX_HOOK_OPERATIONS);
    engine.set_max_string_size(64 * 1024);
    engine.set_max_array_size(16 * 1024);
    engine.set_max_map_size(4 * 1024);
    engine.on_print(|_| {});
    engine.on_debug(|_, _, _| {});
    engine
}

impl GameHost {
    pub fn from_bsp(script: &str, bsp: Arc<Bsp>, seed: u64) -> Result<Self, GameError> {
        let bounds = bsp.models.first().map(|m| Bounds {
            mins: m.mins,
            maxs: m.maxs,
        });
        let entities = bsp.parsed_entities.clone();
        let models = bsp.models.clone();
        Self::with_models(
            script,
            Arc::new(OwnedBspWorld(bsp.clone())),
            bsp,
            &entities,
            &models,
            bounds,
            seed,
        )
    }

    pub fn new(
        script: &str,
        world: Arc<dyn GameWorld>,
        entities: &[Entity],
        models: &[Model],
        world_bounds: Option<Bounds>,
        seed: u64,
    ) -> Result<Self, GameError> {
        let brush = Arc::new(ModelBounds(
            models.iter().map(|m| (m.mins, m.maxs)).collect(),
        ));
        Self::with_models(script, world, brush, entities, models, world_bounds, seed)
    }

    /// Like [`Self::new`] with explicit brush-model queries for the map entity world.
    pub fn with_models(
        script: &str,
        world: Arc<dyn GameWorld>,
        brush: Arc<dyn GameModels>,
        entities: &[Entity],
        models: &[Model],
        world_bounds: Option<Bounds>,
        seed: u64,
    ) -> Result<Self, GameError> {
        let mut engine = sandboxed_engine();
        register_api(&mut engine);
        let ast = engine.compile(script).map_err(script_error)?;
        let hooks = ast.iter_functions().map(|f| f.name.to_string()).collect();
        let game = Game(Arc::new(Mutex::new(Core {
            now: 0,
            roster: Roster::new(),
            map: MapEntities::spawn_for_gametype(entities, &*brush, &Gametype::bomb()),
            models: brush,
            entities: entities
                .iter()
                .map(|e| Dynamic::from(export_entity(e, models)))
                .collect(),
            world,
            world_bounds: world_bounds.unwrap_or(Bounds {
                mins: [-65536.0; 3],
                maxs: [65536.0; 3],
            }),
            rng: Rng::new(seed),
            events: Vec::new(),
        })));
        let mut host = Self {
            game,
            engine,
            ast,
            hooks,
            state: Dynamic::from(Map::new()),
            faults: 0,
        };
        for required in ["init", "tick"] {
            if !host.hooks.contains(required) {
                return Err(script_error(format!("script lacks `{required}`")));
            }
        }
        let g = host.game.clone();
        host.call("init", (g,)).map(drop)?;
        Ok(host)
    }

    fn call(&mut self, name: &str, args: impl FuncArgs) -> Result<Dynamic, GameError> {
        if !self.hooks.contains(name) {
            return Ok(Dynamic::UNIT);
        }
        let options = CallFnOptions::new()
            .eval_ast(false)
            .bind_this_ptr(&mut self.state);
        let result = self.engine.call_fn_with_options::<Dynamic>(
            options,
            &mut Scope::new(),
            &self.ast,
            name,
            args,
        );
        result.map_err(|error| {
            self.faults += 1;
            let mut core = self.game.core();
            let mut fields = BTreeMap::new();
            fields.insert("hook".into(), Value::Str(name.to_owned()));
            fields.insert("tick".into(), Value::Int((core.now / TICK_MS) as i64));
            fields.insert("message".into(), Value::Str(error.to_string()));
            core.events.push(GameEvent {
                kind: "script_fault".into(),
                fields,
            });
            script_error(error)
        })
    }

    /// Number of hook invocations that errored or overran their budget. A hook fault
    /// aborts only that invocation; earlier writes stand and a `script_fault` event is
    /// queued. What to do after repeated faults (disable the gametype on a server) is
    /// the caller's policy.
    pub fn faults(&self) -> u32 {
        self.faults
    }

    pub fn start_match(&mut self) -> Result<(), GameError> {
        self.call("start_match", (self.game.clone(),)).map(drop)
    }

    /// Advances the simulation by one fixed 8 ms tick.
    pub fn tick(&mut self) -> Result<(), GameError> {
        self.game.core().now += TICK_MS;
        self.game.core().run_map();
        self.call("tick", (self.game.clone(),)).map(drop)?;
        self.publish_hud()
    }

    /// Recomputes every player's HUD values once per tick; readers get the published
    /// copy so rendering never runs script code.
    fn publish_hud(&mut self) -> Result<(), GameError> {
        if !self.hooks.contains("hud") {
            return Ok(());
        }
        for id in 0..self.player_count() {
            let values = self.compute_hud(id)?;
            self.game.core().roster.publish_hud(id, values);
        }
        Ok(())
    }

    pub fn add_player(&mut self, team: Team) -> Result<usize, GameError> {
        let id = self.game.core().roster.add(team);
        self.call("on_player_joined", (self.game.clone(), id as i64))
            .map(drop)?;
        Ok(id)
    }

    pub fn change_team(&mut self, id: usize, team: Team) -> Result<(), GameError> {
        self.call(
            "on_team_change",
            (self.game.clone(), id as i64, team.index() as i64),
        )
        .map(drop)
    }

    /// Runs a player command such as `drop` or `primary rl eb`; returns whether the
    /// script handled it.
    pub fn command(&mut self, id: usize, name: &str, args: &str) -> Result<bool, GameError> {
        let result = self.call(
            "on_command",
            (
                self.game.clone(),
                id as i64,
                name.to_owned(),
                args.to_owned(),
            ),
        )?;
        Ok(result.as_bool().unwrap_or(false))
    }

    /// Damage from another player (or the world when `attacker` is `None`).
    pub fn damage(
        &mut self,
        victim: usize,
        attacker: Option<usize>,
        amount: f32,
    ) -> Result<(), GameError> {
        let attacker = attacker.map_or(-1, |a| a as i64);
        self.call(
            "on_damage",
            (
                self.game.clone(),
                victim as i64,
                attacker,
                f64::from(amount),
            ),
        )
        .map(drop)
    }

    pub fn set_player_motion(
        &mut self,
        id: usize,
        origin: Vec3,
        velocity: Vec3,
        yaw: f32,
        pitch: f32,
    ) {
        let mut core = self.game.core();
        let player = core.roster.get_mut(id).expect("no such player");
        if player.alive {
            player.origin = origin;
            player.velocity = velocity;
        }
        player.yaw = yaw;
        player.pitch = pitch;
    }

    pub fn take_events(&mut self) -> Vec<GameEvent> {
        std::mem::take(&mut self.game.core().events)
    }

    pub fn movement_locked(&self) -> bool {
        self.game.core().roster.movement_locked()
    }

    pub fn now_ms(&self) -> u64 {
        self.game.core().now
    }

    pub fn player(&self, id: usize) -> Player {
        self.game
            .core()
            .roster
            .get(id)
            .expect("no such player")
            .clone()
    }

    pub fn player_count(&self) -> usize {
        self.game.core().roster.len()
    }

    /// Gametype-defined HUD values for one player as published at the end of the last
    /// tick (see the script's `hud` hook). Computed on demand before the first tick.
    pub fn hud(&mut self, id: usize) -> Result<BTreeMap<String, Value>, GameError> {
        if let Some(values) = self.game.core().roster.hud(id) {
            return Ok(values.clone());
        }
        self.compute_hud(id)
    }

    fn compute_hud(&mut self, id: usize) -> Result<BTreeMap<String, Value>, GameError> {
        let result = self.call("hud", (self.game.clone(), id as i64))?;
        let map = result.try_cast::<Map>().unwrap_or_default();
        Ok(map
            .iter()
            .filter_map(|(key, value)| Some((key.to_string(), Value::from_dynamic(value)?)))
            .collect())
    }
}
