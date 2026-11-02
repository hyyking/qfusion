//! Local single-player Bomb and Defuse session: runs the engine's gametype host next to the
//! client's movement simulation and draws its HUD values. No window or GPU types here.

use std::collections::BTreeMap;
use std::sync::Arc;

use qf_engine::content::bsp::Bsp;
use qf_engine::game::{BOMB_SCRIPT, GameError, GameHost, Team, Value, WeaponEvent};
use qf_engine::simulation::pmove::{PMFEAT_DASH, PMFEAT_JUMP, PMFEAT_WALLJUMP, PmoveStats};

use crate::hud::{Anchor, Canvas, HAnchor, Rect, Screen, VAnchor, text_size};
use crate::loadout::Loadout;
use crate::local_sim::LocalSim;
use crate::menu::TeamChoice;

const LOCKED_SPEED: i32 = 100;

pub struct BombSession {
    host: GameHost,
    player: usize,
    unlocked_stats: Option<PmoveStats>,
    spectating: bool,
    /// Last accepted loadout choices (the script is authoritative).
    pub loadout: Loadout,
    pub hud: BombHud,
}

impl BombSession {
    pub fn new(bsp: Arc<Bsp>, team: Team, seed: u64) -> Result<Self, GameError> {
        let mut host = GameHost::from_bsp(BOMB_SCRIPT, bsp, seed)?;
        let player = host.add_player(team)?;
        host.start_match()?;
        Ok(Self {
            host,
            player,
            unlocked_stats: None,
            spectating: false,
            loadout: Loadout::default(),
            hud: BombHud::default(),
        })
    }

    /// Runs `ticks` 125 Hz gametype ticks, teleporting the walking player on spawn events.
    /// Returns whether a spawn rewrote the player's look direction.
    pub fn advance(&mut self, ticks: usize, simulation: &mut LocalSim) -> Result<bool, GameError> {
        let mut respawned = false;
        for _ in 0..ticks {
            if let Some(walk) = simulation.walk() {
                let [pitch, yaw, _] = walk.view_angles;
                self.host.set_player_motion(
                    self.player,
                    walk.state.origin,
                    walk.state.velocity,
                    yaw,
                    pitch,
                );
            }
            self.host.tick()?;
            for event in self.host.take_events() {
                match event.kind.as_str() {
                    "spawn" if event.int("player") == Some(self.player as i64) => {
                        if let Some(origin) = event.vec3("origin") {
                            let yaw = event.float("yaw").unwrap_or(0.0) as f32;
                            simulation.edit_walk(|walk| {
                                walk.state.origin = origin;
                                walk.state.velocity = [0.0; 3];
                                walk.view_angles = [0.0, yaw, 0.0];
                                respawned = true;
                                true
                            });
                        }
                    }
                    "round_won" | "match_won" | "bomb_planted" | "bomb_defused"
                    | "bomb_exploded" => println!("{}: {:?}", event.kind, event.fields),
                    _ => {}
                }
            }
        }
        self.hud = BombHud::from_values(&self.host.hud(self.player)?);
        if self.spectating && self.hud.alive {
            // The gametype has no spectator state; keep the player out of the round.
            self.command("suicide", "");
        }
        Ok(respawned)
    }

    /// Applies the gametype's movement lock (round countdown, death) to the pmove stats.
    pub fn apply_movement_lock(&mut self, simulation: &mut LocalSim) {
        let locked = self.host.movement_locked() || !self.hud.alive;
        let unlocked_stats = &mut self.unlocked_stats;
        simulation.edit_walk(|walk| {
            let stats = &mut walk.state.stats;
            match (locked, *unlocked_stats) {
                (true, None) => {
                    *unlocked_stats = Some(*stats);
                    stats.max_speed = LOCKED_SPEED;
                    stats.dash_speed = 0;
                    stats.features &= !(PMFEAT_JUMP | PMFEAT_DASH | PMFEAT_WALLJUMP);
                    true
                }
                (false, Some(saved)) => {
                    *stats = saved;
                    *unlocked_stats = None;
                    true
                }
                _ => false,
            }
        });
    }

    /// Routes the weapon events' damage through the gametype's rules.
    pub fn apply_weapon_events(&mut self, events: &[WeaponEvent]) {
        if let Err(error) = self.host.apply_weapon_events(events) {
            eprintln!("weapon damage: {error}");
        }
    }

    /// Whether the gametype lets the local player shoot (alive, not spectating).
    pub fn can_shoot(&self) -> bool {
        self.hud.alive && !self.spectating
    }

    pub fn command(&mut self, name: &str, args: &str) -> bool {
        match self.host.command(self.player, name, args) {
            Ok(true) => true,
            Ok(false) => {
                println!("{name}: not allowed right now");
                false
            }
            Err(error) => {
                eprintln!("{name}: {error}");
                false
            }
        }
    }

    pub fn set_primary(&mut self, pair: [&'static str; 2]) {
        if let Some(args) = Loadout::primary_args(pair)
            && self.command("primary", &args)
        {
            self.loadout.primary = pair;
        }
    }

    pub fn set_secondary(&mut self, weapon: &'static str) {
        if Loadout::is_valid_secondary(weapon) && self.command("secondary", weapon) {
            self.loadout.secondary = weapon;
        }
    }

    pub fn team_choice(&self) -> TeamChoice {
        if self.spectating {
            return TeamChoice::Spectator;
        }
        match self.host.player(self.player).team {
            Team::Alpha => TeamChoice::Alpha,
            Team::Beta => TeamChoice::Beta,
        }
    }

    /// Joins `choice`. Spectating switches the player to noclip; joining a team returns to walking.
    pub fn set_team(&mut self, choice: TeamChoice, simulation: &mut LocalSim) {
        if choice == self.team_choice() {
            return;
        }
        let team = match choice {
            TeamChoice::Alpha => Team::Alpha,
            TeamChoice::Beta => Team::Beta,
            TeamChoice::Spectator => {
                self.spectating = true;
                simulation.set_noclip(true);
                return;
            }
        };
        self.spectating = false;
        simulation.set_noclip(false);
        if let Err(error) = self.host.change_team(self.player, team) {
            eprintln!("team change: {error}");
        }
    }
}

/// Snapshot of the gametype's HUD values for the local player.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BombHud {
    pub match_state: String,
    pub round_state: String,
    pub round: i64,
    pub clock_ms: i64,
    pub score: [i64; 2],
    pub team: i64,
    pub attacker: bool,
    pub alive: bool,
    pub health: i64,
    pub armor: i64,
    pub carrying: bool,
    pub bomb: String,
    pub site: String,
    /// Letter of the bomb site the carrier currently stands in, empty otherwise.
    pub plant_site: String,
    pub progress: f32,
    pub winner: i64,
}

impl BombHud {
    pub fn from_values(values: &BTreeMap<String, Value>) -> Self {
        let int = |key: &str| match values.get(key) {
            Some(Value::Int(v)) => *v,
            Some(Value::Float(v)) => v.round() as i64,
            _ => 0,
        };
        let text = |key: &str| match values.get(key) {
            Some(Value::Str(v)) => v.clone(),
            _ => String::new(),
        };
        let flag = |key: &str| matches!(values.get(key), Some(Value::Bool(true)));
        Self {
            match_state: text("match_state"),
            round_state: text("round_state"),
            round: int("round"),
            clock_ms: int("clock_ms"),
            score: [int("score_alpha"), int("score_beta")],
            team: int("team"),
            attacker: flag("attacker"),
            alive: flag("alive"),
            health: int("health"),
            armor: int("armor"),
            carrying: flag("carrying"),
            bomb: text("bomb"),
            site: text("bomb_site"),
            plant_site: text("plant_site"),
            progress: match values.get("progress") {
                Some(Value::Float(v)) => *v as f32,
                _ => 0.0,
            },
            winner: match values.get("winner") {
                Some(Value::Int(v)) => *v,
                _ => -1,
            },
        }
    }

    pub fn clock_text(&self) -> String {
        let seconds = (self.clock_ms.max(0) + 999) / 1000;
        format!("{}:{:02}", seconds / 60, seconds % 60)
    }

    pub fn state_text(&self) -> String {
        match (self.match_state.as_str(), self.round_state.as_str()) {
            ("countdown", _) => "MATCH STARTING".into(),
            ("postmatch", _) => "MATCH OVER".into(),
            (_, "pre") => format!("ROUND {} - GET READY", self.round + 1),
            (_, "round") => format!("ROUND {}", self.round),
            (_, "finished" | "post") => "ROUND OVER".into(),
            _ => String::new(),
        }
    }

    pub fn role_text(&self) -> &'static str {
        if self.attacker { "ATTACK" } else { "DEFEND" }
    }

    pub fn bomb_text(&self) -> Option<String> {
        let site = &self.site;
        match self.bomb.as_str() {
            "carried" if self.carrying && !self.plant_site.is_empty() => Some(format!(
                "IN SITE {} - STAND STILL ON THE GROUND TO PLANT",
                self.plant_site
            )),
            "carried" if self.carrying => Some("YOU HAVE THE BOMB - G TO DROP".into()),
            "carried" => Some("BOMB CARRIED".into()),
            "dropping" | "dropped" => Some("BOMB DROPPED".into()),
            "planting" | "planted" => Some(format!("ARMING SITE {site}")),
            "armed" => Some(format!("BOMB ARMED SITE {site}")),
            "defused" => Some("BOMB DEFUSED".into()),
            "exploded" => Some("BOMB EXPLODED".into()),
            _ => None,
        }
    }

    pub fn draw(&self, screen: &Screen, canvas: &mut Canvas) {
        let pixel = 2.0 * screen.scale;
        let white = [1.0, 1.0, 1.0, 1.0];
        let centered = |canvas: &mut Canvas, text: &str, y: f32, scale: f32, color| {
            let anchor = Anchor {
                horizontal: HAnchor::Center,
                vertical: VAnchor::Top,
                margin: [0.0, 0.0],
            };
            let size = text_size(text.chars().count(), pixel * scale);
            let [x, _] = anchor.place(screen, size);
            canvas.shadowed_text(text, [x, y * screen.scale], pixel * scale, color);
        };
        let score = format!("ALPHA {} - {} BETA", self.score[0], self.score[1]);
        centered(canvas, &score, 8.0, 1.0, white);
        centered(canvas, &self.state_text(), 28.0, 1.0, white);
        centered(canvas, &self.clock_text(), 48.0, 2.0, white);
        centered(canvas, self.role_text(), 80.0, 1.0, [0.8, 0.8, 0.8, 1.0]);
        if let Some(text) = self.bomb_text() {
            let color = match self.bomb.as_str() {
                "armed" | "exploded" => [1.0, 0.3, 0.2, 1.0],
                "defused" => [0.3, 1.0, 0.4, 1.0],
                _ => [1.0, 0.9, 0.3, 1.0],
            };
            centered(canvas, &text, 100.0, 1.0, color);
        }
        if self.progress > 0.0 {
            let width = 200.0 * screen.scale;
            let height = 8.0 * screen.scale;
            let x = (screen.width - width) * 0.5;
            let y = 124.0 * screen.scale;
            canvas.rects.push(Rect {
                x,
                y,
                w: width,
                h: height,
                color: [0.0, 0.0, 0.0, 0.6],
            });
            canvas.rects.push(Rect {
                x,
                y,
                w: width * self.progress.clamp(0.0, 1.0),
                h: height,
                color: [1.0, 0.9, 0.3, 0.9],
            });
        }
        if self.winner >= 0 && self.match_state == "postmatch" {
            let name = if self.winner == 0 { "ALPHA" } else { "BETA" };
            centered(canvas, &format!("{name} WINS"), 150.0, 2.0, white);
        }
        let status = if self.alive {
            format!("HP {}  AR {}", self.health, self.armor)
        } else {
            "DEAD".into()
        };
        let anchor = Anchor {
            horizontal: HAnchor::Left,
            vertical: VAnchor::Bottom,
            margin: [16.0, 16.0],
        };
        let size = text_size(status.chars().count(), pixel * 2.0);
        let health_color = if self.health <= 25 {
            [1.0, 0.3, 0.2, 1.0]
        } else {
            white
        };
        canvas.shadowed_text(
            &status,
            anchor.place(screen, size),
            pixel * 2.0,
            health_color,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(pairs: &[(&str, Value)]) -> BTreeMap<String, Value> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    const SCREEN: Screen = Screen {
        width: 1280.0,
        height: 720.0,
        scale: 1.0,
    };

    #[test]
    fn parses_script_values() {
        let hud = BombHud::from_values(&values(&[
            ("round_state", Value::Str("round".into())),
            ("clock_ms", Value::Int(59_001)),
            ("health", Value::Float(99.6)),
            ("alive", Value::Bool(true)),
            ("progress", Value::Float(0.5)),
        ]));
        assert_eq!(hud.clock_text(), "1:00");
        assert_eq!(hud.health, 100);
        assert!(hud.alive);
        assert_eq!(hud.progress, 0.5);
    }

    #[test]
    fn bomb_status_text() {
        let mut hud = BombHud::default();
        assert_eq!(hud.bomb_text(), None);
        hud.bomb = "carried".into();
        hud.carrying = true;
        assert_eq!(hud.bomb_text().unwrap(), "YOU HAVE THE BOMB - G TO DROP");
        hud.plant_site = "B".into();
        assert!(hud.bomb_text().unwrap().starts_with("IN SITE B"));
        hud.bomb = "armed".into();
        hud.site = "A".into();
        assert_eq!(hud.bomb_text().unwrap(), "BOMB ARMED SITE A");
    }

    #[test]
    fn draws_inside_the_screen() {
        let hud = BombHud {
            match_state: "playing".into(),
            round_state: "round".into(),
            round: 3,
            clock_ms: 30_000,
            alive: true,
            health: 100,
            armor: 100,
            bomb: "planting".into(),
            site: "B".into(),
            progress: 0.4,
            ..BombHud::default()
        };
        let mut canvas = Canvas::default();
        hud.draw(&SCREEN, &mut canvas);
        assert!(canvas.rects.len() > 100);
        for r in &canvas.rects {
            assert!(r.x >= 0.0 && r.y >= 0.0);
            assert!(r.x + r.w <= SCREEN.width && r.y + r.h <= SCREEN.height);
        }
    }
}
