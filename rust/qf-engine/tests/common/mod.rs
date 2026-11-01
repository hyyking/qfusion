#![allow(dead_code)]

use qf_engine::game::{GameEvent, GameHost, Value};
use qf_engine::simulation::pmove::Vec3;

/// Drives a `GameHost` at the fixed tick with scripted player positions.
pub struct Harness {
    pub host: GameHost,
    pub events: Vec<GameEvent>,
    pub held: Vec<Option<Vec3>>,
}

impl Harness {
    pub fn new(host: GameHost, players: usize) -> Self {
        Self {
            host,
            events: Vec::new(),
            held: vec![None; players],
        }
    }

    pub fn tick(&mut self) {
        for (id, position) in self.held.clone().into_iter().enumerate() {
            if let Some(position) = position {
                self.host
                    .set_player_motion(id, position, [0.0; 3], 0.0, 0.0);
            }
        }
        self.host.tick().unwrap();
        self.events.extend(self.host.take_events());
    }

    pub fn run_until(&mut self, limit_ms: u64, mut done: impl FnMut(&mut Harness) -> bool) {
        for _ in 0..limit_ms / 8 {
            if done(self) {
                return;
            }
            self.tick();
        }
        panic!("condition not reached; events: {:#?}", self.events);
    }

    pub fn hud_text(&mut self, id: usize, key: &str) -> String {
        match self.host.hud(id).unwrap().remove(key) {
            Some(Value::Str(text)) => text,
            other => panic!("hud {key} is {other:?}"),
        }
    }

    pub fn round_state(&mut self) -> String {
        self.hud_text(0, "round_state")
    }

    pub fn bomb_state(&mut self) -> String {
        self.hud_text(0, "bomb")
    }

    pub fn start_round(&mut self) {
        self.run_until(20_000, |h| h.round_state() == "round");
    }

    pub fn find(&self, kind: &str) -> Vec<&GameEvent> {
        self.events.iter().filter(|e| e.kind == kind).collect()
    }

    /// `(team, reason)` of the first round result.
    pub fn round_won(&self) -> Option<(i64, String)> {
        let event = self.find("round_won").into_iter().next()?;
        Some((event.int("team")?, event.text("reason")?.to_owned()))
    }

    pub fn spawn_of(&self, id: usize) -> Vec3 {
        self.events
            .iter()
            .rev()
            .filter(|e| e.kind == "spawn" && e.int("player") == Some(id as i64))
            .find_map(|e| e.vec3("origin"))
            .unwrap()
    }
}
