//! Input-log recording and replay of a [`GameHost`] match.
//!
//! A [`GameLog`] stores only what enters the game from outside: the seed, the script
//! hash, joins, team changes, commands, damage, player motion and tick boundaries,
//! plus a [`GameHost::state_hash`] checkpoint every N ticks. No state is stored;
//! [`GameLog::replay`] re-runs the host and fails at the first checkpoint whose hash
//! differs, which pins a determinism bug to a tick.
//!
//! The text format is line based and versioned (`qfusion-game-recording 1`). Floats use
//! Rust's shortest round-trip formatting and free text is hex encoded, so parsing
//! reproduces the exact values. This is an internal test/debug tool, not a
//! player-facing demo format.

use std::fmt::{self, Write as _};

use super::combat::Team;
use super::host::{GameError, GameHost, TICK_MS};
use crate::simulation::pmove::Vec3;

pub const FORMAT_NAME: &str = "qfusion-game-recording";
pub const FORMAT_VERSION: u32 = 1;

/// FNV-1a 64, used for the script hash.
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// One external input to the game.
#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    Join(Team),
    StartMatch,
    ChangeTeam {
        id: usize,
        team: Team,
    },
    Command {
        id: usize,
        name: String,
        args: String,
    },
    Damage {
        victim: usize,
        attacker: Option<usize>,
        amount: f32,
    },
    Motion {
        id: usize,
        origin: Vec3,
        velocity: Vec3,
        yaw: f32,
        pitch: f32,
    },
    Tick,
}

/// A recorded match.
#[derive(Clone, Debug, PartialEq)]
pub struct GameLog {
    pub seed: u64,
    pub script_hash: u64,
    /// A checkpoint is stored after every this-many ticks.
    pub checkpoint_every: u64,
    pub ops: Vec<Op>,
    /// `(tick, state_hash)` in tick order.
    pub checkpoints: Vec<(u64, u64)>,
}

#[derive(Debug)]
pub enum ReplayError {
    /// The host runs a different script than the recording.
    ScriptMismatch {
        recorded: u64,
        actual: u64,
    },
    /// The host failed to apply an input.
    Game(GameError),
    /// The state diverged at the first differing checkpoint.
    Diverged {
        tick: u64,
        recorded: u64,
        actual: u64,
    },
    Parse(String),
}

impl fmt::Display for ReplayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ScriptMismatch { recorded, actual } => {
                write!(f, "script hash {actual:016x} != recorded {recorded:016x}")
            }
            Self::Game(error) => write!(f, "{error}"),
            Self::Diverged {
                tick,
                recorded,
                actual,
            } => write!(
                f,
                "state diverged at tick {tick}: {actual:016x} != recorded {recorded:016x}"
            ),
            Self::Parse(message) => write!(f, "bad recording: {message}"),
        }
    }
}

impl std::error::Error for ReplayError {}

impl From<GameError> for ReplayError {
    fn from(error: GameError) -> Self {
        Self::Game(error)
    }
}

fn apply(host: &mut GameHost, op: &Op) -> Result<(), GameError> {
    match op {
        Op::Join(team) => host.add_player(*team).map(drop),
        Op::StartMatch => host.start_match(),
        Op::ChangeTeam { id, team } => host.change_team(*id, *team),
        Op::Command { id, name, args } => host.command(*id, name, args).map(drop),
        Op::Damage {
            victim,
            attacker,
            amount,
        } => host.damage(*victim, *attacker, *amount),
        Op::Motion {
            id,
            origin,
            velocity,
            yaw,
            pitch,
        } => {
            host.set_player_motion(*id, *origin, *velocity, *yaw, *pitch);
            Ok(())
        }
        Op::Tick => host.tick(),
    }
}

impl GameLog {
    /// Re-runs the log on a freshly built `host` (same script, map and seed) and checks
    /// every checkpoint. Script faults while replaying are errors, as they were
    /// recorded as successful inputs.
    pub fn replay(&self, host: &mut GameHost, script: &str) -> Result<(), ReplayError> {
        let actual = fnv1a64(script.as_bytes());
        if actual != self.script_hash {
            return Err(ReplayError::ScriptMismatch {
                recorded: self.script_hash,
                actual,
            });
        }
        let mut checkpoints = self.checkpoints.iter().peekable();
        let mut tick = 0_u64;
        for op in &self.ops {
            apply(host, op)?;
            if matches!(op, Op::Tick) {
                tick += 1;
                while let Some(&&(at, recorded)) = checkpoints.peek() {
                    if at > tick {
                        break;
                    }
                    checkpoints.next();
                    let actual = host.state_hash();
                    if at == tick && actual != recorded {
                        return Err(ReplayError::Diverged {
                            tick,
                            recorded,
                            actual,
                        });
                    }
                }
            }
        }
        Ok(())
    }

    pub fn to_text(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "{FORMAT_NAME} {FORMAT_VERSION}");
        let _ = writeln!(out, "tick_ms {TICK_MS}");
        let _ = writeln!(out, "seed {}", self.seed);
        let _ = writeln!(out, "script {:016x}", self.script_hash);
        let _ = writeln!(out, "checkpoint_every {}", self.checkpoint_every);
        let mut tick = 0_u64;
        let mut checkpoints = self.checkpoints.iter().peekable();
        for op in &self.ops {
            match op {
                Op::Join(team) => {
                    let _ = writeln!(out, "join {}", team.index());
                }
                Op::StartMatch => out.push_str("start\n"),
                Op::ChangeTeam { id, team } => {
                    let _ = writeln!(out, "team {id} {}", team.index());
                }
                Op::Command { id, name, args } => {
                    let _ = writeln!(out, "cmd {id} {} {}", hex(name), hex(args));
                }
                Op::Damage {
                    victim,
                    attacker,
                    amount,
                } => {
                    let attacker = attacker.map_or(-1, |a| a as i64);
                    let _ = writeln!(out, "damage {victim} {attacker} {amount:?}");
                }
                Op::Motion {
                    id,
                    origin,
                    velocity,
                    yaw,
                    pitch,
                } => {
                    let _ = writeln!(
                        out,
                        "motion {id} {:?} {:?} {:?} {:?} {:?} {:?} {yaw:?} {pitch:?}",
                        origin[0], origin[1], origin[2], velocity[0], velocity[1], velocity[2]
                    );
                }
                Op::Tick => {
                    tick += 1;
                    out.push_str("tick\n");
                    while let Some(&&(at, hash)) = checkpoints.peek() {
                        if at > tick {
                            break;
                        }
                        checkpoints.next();
                        let _ = writeln!(out, "check {at} {hash:016x}");
                    }
                }
            }
        }
        out
    }

    pub fn from_text(text: &str) -> Result<Self, ReplayError> {
        let bad = |line: &str| ReplayError::Parse(format!("line `{line}`"));
        let mut lines = text.lines();
        let header = lines.next().ok_or_else(|| bad(""))?;
        if header != format!("{FORMAT_NAME} {FORMAT_VERSION}") {
            return Err(ReplayError::Parse(format!("unsupported header `{header}`")));
        }
        let mut log = Self {
            seed: 0,
            script_hash: 0,
            checkpoint_every: 0,
            ops: Vec::new(),
            checkpoints: Vec::new(),
        };
        for line in lines.filter(|l| !l.trim().is_empty()) {
            let parts: Vec<&str> = line.split(' ').collect();
            let int = |i: usize| -> Result<usize, ReplayError> {
                parts
                    .get(i)
                    .and_then(|s| s.parse().ok())
                    .ok_or_else(|| bad(line))
            };
            let float = |i: usize| -> Result<f32, ReplayError> {
                parts
                    .get(i)
                    .and_then(|s| s.parse().ok())
                    .ok_or_else(|| bad(line))
            };
            let team = |i: usize| -> Result<Team, ReplayError> {
                Team::from_index(int(i)? as i64).ok_or_else(|| bad(line))
            };
            let hash = |i: usize| -> Result<u64, ReplayError> {
                parts
                    .get(i)
                    .and_then(|s| u64::from_str_radix(s, 16).ok())
                    .ok_or_else(|| bad(line))
            };
            match parts[0] {
                "tick_ms" => {
                    if int(1)? as u64 != TICK_MS {
                        return Err(ReplayError::Parse("tick length differs".into()));
                    }
                }
                "seed" => log.seed = int(1)? as u64,
                "script" => log.script_hash = hash(1)?,
                "checkpoint_every" => log.checkpoint_every = int(1)? as u64,
                "join" => log.ops.push(Op::Join(team(1)?)),
                "start" => log.ops.push(Op::StartMatch),
                "team" => log.ops.push(Op::ChangeTeam {
                    id: int(1)?,
                    team: team(2)?,
                }),
                "cmd" => log.ops.push(Op::Command {
                    id: int(1)?,
                    name: unhex(parts.get(2).ok_or_else(|| bad(line))?).ok_or_else(|| bad(line))?,
                    args: unhex(parts.get(3).ok_or_else(|| bad(line))?).ok_or_else(|| bad(line))?,
                }),
                "damage" => {
                    let attacker: i64 = parts
                        .get(2)
                        .and_then(|s| s.parse().ok())
                        .ok_or_else(|| bad(line))?;
                    log.ops.push(Op::Damage {
                        victim: int(1)?,
                        attacker: usize::try_from(attacker).ok(),
                        amount: float(3)?,
                    });
                }
                "motion" => log.ops.push(Op::Motion {
                    id: int(1)?,
                    origin: [float(2)?, float(3)?, float(4)?],
                    velocity: [float(5)?, float(6)?, float(7)?],
                    yaw: float(8)?,
                    pitch: float(9)?,
                }),
                "tick" => log.ops.push(Op::Tick),
                "check" => log.checkpoints.push((int(1)? as u64, hash(2)?)),
                _ => return Err(bad(line)),
            }
        }
        Ok(log)
    }
}

fn hex(text: &str) -> String {
    if text.is_empty() {
        return "-".into();
    }
    text.bytes().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Option<String> {
    if text == "-" {
        return Some(String::new());
    }
    if !text.len().is_multiple_of(2) {
        return None;
    }
    let bytes: Option<Vec<u8>> = (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok())
        .collect();
    String::from_utf8(bytes?).ok()
}

/// Drives a [`GameHost`] while logging every input.
pub struct Recorder {
    pub host: GameHost,
    log: GameLog,
    ticks: u64,
}

impl Recorder {
    /// `script` and `seed` must be the ones `host` was built with.
    pub fn new(host: GameHost, script: &str, seed: u64, checkpoint_every: u64) -> Self {
        Self {
            host,
            log: GameLog {
                seed,
                script_hash: fnv1a64(script.as_bytes()),
                checkpoint_every: checkpoint_every.max(1),
                ops: Vec::new(),
                checkpoints: Vec::new(),
            },
            ticks: 0,
        }
    }

    /// Applies `op` to the host and logs it. Inputs the host rejects are not logged.
    pub fn input(&mut self, op: Op) -> Result<(), GameError> {
        apply(&mut self.host, &op)?;
        if matches!(op, Op::Tick) {
            self.ticks += 1;
            if self.ticks.is_multiple_of(self.log.checkpoint_every) {
                self.log
                    .checkpoints
                    .push((self.ticks, self.host.state_hash()));
            }
        }
        self.log.ops.push(op);
        Ok(())
    }

    pub fn finish(self) -> GameLog {
        self.log
    }
}
