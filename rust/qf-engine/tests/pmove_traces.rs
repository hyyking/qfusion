//! Replays the recorded legacy `Pmove` traces (`tests/fixtures/pmove_125hz.csv`,
//! produced by `source/common/tools/pmove_trace.cpp`) through the Rust movement
//! implementation and compares every step.
//!
//! Exact invariants (compared with `==`): event sequence and parameters, movement
//! flags, grounded/water/ladder state, the ordered touch list and whether a step
//! height was reported. Trajectory values (origin, velocity, non-zero step
//! height) use per-component float tolerances. The fixture world is the harness'
//! own axis-aligned plane world, so these tests exercise movement integration only;
//! BSP collision is validated separately in `tests/bsp.rs` and `tests/pmove_bsp.rs`.

use qf_engine::simulation::pmove::*;

const FIXTURE: &str = include_str!("fixtures/pmove_125hz.csv");

/// Same-compiler tolerance from `docs/movement-reference.md`.
const DEFAULT_TOLERANCE: f32 = 0.05;
/// The dash scenarios differ from the legacy build by up to ~0.033 velocity units (the
/// legacy x64 build normalises the dash direction with the SSE `rsqrtss` estimate; Rust
/// uses an exact square root), which stays inside the default tolerance.
const DASH_TOLERANCE: f32 = DEFAULT_TOLERANCE;

struct CollisionPlane {
    axis: usize,
    normal: f32,
    distance: f32,
    surface_flags: u32,
    entity: i32,
}

/// The world used by the legacy harness: a flat floor, an optional wall and an
/// optional water volume spanning z in [0, 256].
struct HarnessWorld {
    planes: Vec<CollisionPlane>,
    water: bool,
}

impl PmoveWorld for HarnessWorld {
    fn trace(&self, start: Vec3, mins: Vec3, maxs: Vec3, end: Vec3, _mask: u32) -> PmTrace {
        let mut trace = PmTrace::miss(end);
        for plane in &self.planes {
            let support = if plane.normal > 0.0 {
                mins[plane.axis]
            } else {
                -maxs[plane.axis]
            };
            let start_distance = plane.normal * start[plane.axis] + support - plane.distance;
            let end_distance = plane.normal * end[plane.axis] + support - plane.distance;

            if start_distance < 0.0 {
                trace.start_solid = true;
                trace.all_solid = end_distance < 0.0;
                continue;
            }
            if end_distance >= 0.0 || end_distance == start_distance {
                continue;
            }
            let fraction = start_distance / (start_distance - end_distance);
            if fraction >= trace.fraction {
                continue;
            }
            trace.fraction = fraction;
            trace.end_position = std::array::from_fn(|i| start[i] + fraction * (end[i] - start[i]));
            trace.plane_normal = [0.0; 3];
            trace.plane_normal[plane.axis] = plane.normal;
            trace.surface_flags = plane.surface_flags;
            trace.contents = CONTENTS_SOLID;
            trace.entity = plane.entity;
            trace.all_solid = false;
        }
        trace
    }

    fn point_contents(&self, point: Vec3) -> u32 {
        if self.water && (0.0..=256.0).contains(&point[2]) {
            CONTENTS_WATER
        } else {
            0
        }
    }
}

struct Case {
    name: &'static str,
    start_x: f32,
    start_z: f32,
    start_velocity_x: f32,
    start_velocity_z: f32,
    wall: bool,
    water: bool,
    ladder: bool,
    tolerance: f32,
}

const CASES: &[Case] = &[
    case(
        "jump_tap",
        0.0,
        24.0,
        0.0,
        0.0,
        false,
        false,
        false,
        DEFAULT_TOLERANCE,
    ),
    case(
        "jump_hold",
        0.0,
        24.0,
        0.0,
        0.0,
        false,
        false,
        false,
        DEFAULT_TOLERANCE,
    ),
    case(
        "air_direction_change",
        0.0,
        24.0,
        0.0,
        0.0,
        false,
        false,
        false,
        DEFAULT_TOLERANCE,
    ),
    case(
        "dash_wall_hit",
        0.0,
        24.0,
        0.0,
        0.0,
        true,
        false,
        false,
        DASH_TOLERANCE,
    ),
    case(
        "walljump_after_dash",
        0.0,
        24.0,
        0.0,
        0.0,
        true,
        false,
        false,
        DASH_TOLERANCE,
    ),
    case(
        "land_slow",
        0.0,
        180.0,
        90.0,
        -100.0,
        false,
        false,
        false,
        DEFAULT_TOLERANCE,
    ),
    case(
        "land_fast",
        0.0,
        180.0,
        360.0,
        -500.0,
        false,
        false,
        false,
        DEFAULT_TOLERANCE,
    ),
    case(
        "water_swim",
        0.0,
        24.0,
        0.0,
        0.0,
        false,
        true,
        false,
        DEFAULT_TOLERANCE,
    ),
    case(
        "ladder_climb",
        94.0,
        40.0,
        0.0,
        0.0,
        true,
        false,
        true,
        DEFAULT_TOLERANCE,
    ),
];

#[allow(clippy::too_many_arguments)]
const fn case(
    name: &'static str,
    start_x: f32,
    start_z: f32,
    start_velocity_x: f32,
    start_velocity_z: f32,
    wall: bool,
    water: bool,
    ladder: bool,
    tolerance: f32,
) -> Case {
    Case {
        name,
        start_x,
        start_z,
        start_velocity_x,
        start_velocity_z,
        wall,
        water,
        ladder,
        tolerance,
    }
}

struct Row {
    scenario: String,
    step: usize,
    command: PmoveCommand,
    position: Vec3,
    velocity: Vec3,
    grounded: bool,
    water_level: u8,
    ladder: bool,
    flags: u32,
    events: String,
    touches: String,
    step_height: f32,
}

fn split_csv(line: &str) -> Vec<&str> {
    let mut fields = Vec::new();
    let mut start = 0;
    let mut quoted = false;
    for (index, byte) in line.bytes().enumerate() {
        match byte {
            b'"' => quoted = !quoted,
            b',' if !quoted => {
                fields.push(&line[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    fields.push(&line[start..]);
    fields
}

fn rows() -> Vec<Row> {
    FIXTURE
        .lines()
        .skip(1)
        .map(|line| {
            let f = split_csv(line);
            let float = |i: usize| f[i].parse::<f32>().unwrap();
            let int = |i: usize| f[i].parse::<i32>().unwrap();
            assert_eq!(int(1), 125, "fixture is recorded at the 125 Hz tick");
            Row {
                scenario: f[0].to_string(),
                step: int(2) as usize,
                command: PmoveCommand {
                    msec: int(4) as u8,
                    buttons: int(5) as u8,
                    forward_move: int(6) as i8,
                    side_move: int(7) as i8,
                    up_move: int(8) as i8,
                    angles: [int(9) as i16, int(10) as i16, int(11) as i16],
                },
                position: [float(12), float(13), float(14)],
                velocity: [float(15), float(16), float(17)],
                grounded: int(18) != 0,
                water_level: int(19) as u8,
                ladder: int(20) != 0,
                flags: u32::from_str_radix(f[21].trim_start_matches("0x"), 16).unwrap(),
                events: f[22].trim_matches('"').to_string(),
                touches: f[23].trim_matches('"').to_string(),
                step_height: float(24),
            }
        })
        .collect()
}

fn event_name(event: PmEvent) -> &'static str {
    match event {
        PmEvent::Dash => "DASH",
        PmEvent::WallJump => "WALLJUMP",
        PmEvent::WallJumpFailed => "WALLJUMP_FAILED",
        PmEvent::DoubleJump => "DOUBLEJUMP",
        PmEvent::Jump => "JUMP",
        PmEvent::JumpPad => "JUMP_PAD",
        PmEvent::Fall => "FALL",
    }
}

fn world_for(case: &Case) -> HarnessWorld {
    let mut planes = vec![CollisionPlane {
        axis: 2,
        normal: 1.0,
        distance: 0.0,
        surface_flags: 0,
        entity: 1,
    }];
    if case.wall {
        planes.push(CollisionPlane {
            axis: 0,
            normal: -1.0,
            distance: -110.0,
            surface_flags: if case.ladder { SURF_LADDER } else { 0 },
            entity: 2,
        });
    }
    HarnessWorld {
        planes,
        water: case.water,
    }
}

fn initial_state(case: &Case) -> PmoveState {
    let mut state = PmoveState::new([case.start_x, 0.0, case.start_z]);
    state.velocity = [case.start_velocity_x, 0.0, case.start_velocity_z];
    state.stats.features = PMFEAT_JUMP
        | PMFEAT_DASH
        | PMFEAT_WALLJUMP
        | PMFEAT_AIRCONTROL
        | PMFEAT_FWDBUNNY
        | PMFEAT_CROUCH
        | PMFEAT_CROUCHSLIDING;
    state
}

#[derive(Default)]
struct Report {
    exact_mismatches: Vec<String>,
    tolerance_mismatches: Vec<String>,
    max_position_error: f32,
    max_velocity_error: f32,
    steps: usize,
}

fn replay(case: &Case, rows: &[&Row]) -> Report {
    let world = world_for(case);
    let rules = PmoveRules::default();
    let mut state = initial_state(case);
    let mut report = Report::default();

    for row in rows {
        let result = pmove(&world, &rules, &mut state, &row.command);
        report.steps += 1;
        let at = format!("{} step {}", case.name, row.step);

        let events = result
            .events
            .iter()
            .map(|e| format!("{}[{}]:{}", event_name(e.event), e.event as u8, e.parameter))
            .collect::<Vec<_>>()
            .join(";");
        let touches = result
            .touch_entities
            .iter()
            .map(i32::to_string)
            .collect::<Vec<_>>()
            .join(";");
        let mut exact = |what: &str, ok: bool, rust: String, legacy: String| {
            if !ok {
                report
                    .exact_mismatches
                    .push(format!("{at}: {what}: rust {rust}, legacy {legacy}"));
            }
        };
        exact(
            "events",
            events == row.events,
            events.clone(),
            row.events.clone(),
        );
        exact(
            "flags",
            state.flags == row.flags,
            format!("{:#x}", state.flags),
            format!("{:#x}", row.flags),
        );
        exact(
            "grounded",
            state.on_ground() == row.grounded,
            state.on_ground().to_string(),
            row.grounded.to_string(),
        );
        exact(
            "water level",
            result.water_level == row.water_level,
            result.water_level.to_string(),
            row.water_level.to_string(),
        );
        exact(
            "ladder",
            result.ladder == row.ladder,
            result.ladder.to_string(),
            row.ladder.to_string(),
        );
        exact(
            "touches",
            touches == row.touches,
            touches.clone(),
            row.touches.clone(),
        );
        exact(
            "step reported",
            (result.step == 0.0) == (row.step_height == 0.0),
            result.step.to_string(),
            row.step_height.to_string(),
        );

        for axis in 0..3 {
            let position_error = (state.origin[axis] - row.position[axis]).abs();
            let velocity_error = (state.velocity[axis] - row.velocity[axis]).abs();
            report.max_position_error = report.max_position_error.max(position_error);
            report.max_velocity_error = report.max_velocity_error.max(velocity_error);
            if position_error > case.tolerance || velocity_error > case.tolerance {
                report.tolerance_mismatches.push(format!(
                    "{at}: axis {axis}: position {} vs {}, velocity {} vs {}",
                    state.origin[axis],
                    row.position[axis],
                    state.velocity[axis],
                    row.velocity[axis]
                ));
            }
        }
        if result.step != 0.0 && row.step_height != 0.0 {
            assert!(
                (result.step - row.step_height).abs() <= case.tolerance,
                "{at}: step height"
            );
        }
    }
    report
}

fn run_case(name: &str) {
    let case = CASES.iter().find(|c| c.name == name).unwrap();
    let all = rows();
    let rows: Vec<&Row> = all.iter().filter(|r| r.scenario == name).collect();
    assert_eq!(rows.len(), 200, "{name}: 1.6 s at 8 ms per command");
    for (index, row) in rows.iter().enumerate() {
        assert_eq!(row.step, index);
        assert_eq!(row.command.msec, 8);
    }

    let report = replay(case, &rows);
    println!(
        "{name}: {} steps, exact mismatches {}, tolerance mismatches {}, max |position error| {:.4}, max |velocity error| {:.4}",
        report.steps,
        report.exact_mismatches.len(),
        report.tolerance_mismatches.len(),
        report.max_position_error,
        report.max_velocity_error
    );
    assert!(
        report.exact_mismatches.is_empty(),
        "exact invariant mismatches (movement integration):\n{}",
        report.exact_mismatches[..report.exact_mismatches.len().min(10)].join("\n")
    );
    assert!(
        report.tolerance_mismatches.is_empty(),
        "trajectory outside tolerance {} (movement integration):\n{}",
        case.tolerance,
        report.tolerance_mismatches[..report.tolerance_mismatches.len().min(10)].join("\n")
    );
}

macro_rules! trace_tests {
    ($($test:ident => $name:literal),* $(,)?) => {
        $(#[test] fn $test() { run_case($name); })*
    };
}

trace_tests! {
    jump_tap => "jump_tap",
    jump_hold => "jump_hold",
    air_direction_change => "air_direction_change",
    dash_wall_hit => "dash_wall_hit",
    walljump_after_dash => "walljump_after_dash",
    land_slow => "land_slow",
    land_fast => "land_fast",
    water_swim => "water_swim",
    ladder_climb => "ladder_climb",
}

#[test]
fn fixture_covers_every_case() {
    let all = rows();
    for case in CASES {
        assert!(all.iter().any(|row| row.scenario == case.name));
    }
    assert_eq!(all.len(), CASES.len() * 200);
}
