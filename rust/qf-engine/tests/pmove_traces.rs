//! Replays the recorded legacy `Pmove` traces (`tests/fixtures/pmove_traces.csv`,
//! produced by `source/common/tools/pmove_trace.cpp`) through the Rust movement
//! implementation and compares every step.
//!
//! Exact invariants (compared with `==`): event sequence and parameters, movement
//! flags, grounded/water/ladder state, the ordered touch list and whether a step
//! height was reported. Trajectory values (origin, velocity, non-zero step
//! height and view height) use per-component float tolerances. The fixture world is the
//! harness' own plane/box world, so these tests exercise movement integration only;
//! BSP collision is validated separately in `tests/bsp.rs` and `tests/pmove_bsp.rs`.

use qf_engine::simulation::pmove::*;

const FIXTURE: &str = include_str!("fixtures/pmove_traces.csv");
const RATES: &[f32] = &[125.0, 100.0, 62.5, 60.0, 50.0, 30.0];

/// Same-compiler tolerance from `docs/movement-reference.md`.
const DEFAULT_TOLERANCE: f32 = 0.05;
/// The dash scenarios differ from the legacy build by up to ~0.033 velocity units (the
/// legacy x64 build normalises the dash direction with the SSE `rsqrtss` estimate; Rust
/// uses an exact square root), which stays inside the default tolerance.
const DASH_TOLERANCE: f32 = DEFAULT_TOLERANCE;

struct CollisionPlane {
    normal: Vec3,
    distance: f32,
    surface_flags: u32,
    entity: i32,
}

struct CollisionBox {
    mins: Vec3,
    maxs: Vec3,
    surface_flags: u32,
    entity: i32,
}

/// The simple geometries used by the legacy harness; BSP collision is tested separately.
struct HarnessWorld {
    planes: Vec<CollisionPlane>,
    boxes: Vec<CollisionBox>,
    water: bool,
}

impl PmoveWorld for HarnessWorld {
    fn trace(&self, start: Vec3, mins: Vec3, maxs: Vec3, end: Vec3, _mask: u32) -> PmTrace {
        let mut trace = PmTrace::miss(end);
        for plane in &self.planes {
            let mut support = 0.0;
            let mut start_distance = -plane.distance;
            let mut end_distance = -plane.distance;
            for axis in 0..3 {
                support += plane.normal[axis]
                    * if plane.normal[axis] > 0.0 {
                        mins[axis]
                    } else {
                        maxs[axis]
                    };
                start_distance += plane.normal[axis] * start[axis];
                end_distance += plane.normal[axis] * end[axis];
            }
            start_distance += support;
            end_distance += support;

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
            trace.plane_normal = plane.normal;
            trace.surface_flags = plane.surface_flags;
            trace.contents = CONTENTS_SOLID;
            trace.entity = plane.entity;
            trace.all_solid = false;
        }

        for obstacle in &self.boxes {
            let expanded_mins: Vec3 = std::array::from_fn(|i| obstacle.mins[i] - maxs[i]);
            let expanded_maxs: Vec3 = std::array::from_fn(|i| obstacle.maxs[i] - mins[i]);
            let start_inside =
                (0..3).all(|i| start[i] > expanded_mins[i] && start[i] < expanded_maxs[i]);
            let end_inside = (0..3).all(|i| end[i] > expanded_mins[i] && end[i] < expanded_maxs[i]);
            let mut enter = f32::NEG_INFINITY;
            let mut leave: f32 = 1.0;
            let mut hit_axis = None;
            let mut hit_normal = 0.0;
            let mut intersects = true;

            for axis in 0..3 {
                let delta = end[axis] - start[axis];
                if delta == 0.0 {
                    if start[axis] <= expanded_mins[axis] || start[axis] >= expanded_maxs[axis] {
                        intersects = false;
                    }
                    continue;
                }

                let mut near_distance = (expanded_mins[axis] - start[axis]) / delta;
                let mut far_distance = (expanded_maxs[axis] - start[axis]) / delta;
                let mut normal = -1.0;
                if near_distance > far_distance {
                    std::mem::swap(&mut near_distance, &mut far_distance);
                    normal = 1.0;
                }
                if near_distance > enter {
                    enter = near_distance;
                    hit_axis = Some(axis);
                    hit_normal = normal;
                }
                leave = leave.min(far_distance);
                if enter > leave {
                    intersects = false;
                }
            }

            if start_inside {
                trace.start_solid = true;
                trace.all_solid = end_inside;
                continue;
            }
            let Some(axis) = hit_axis else {
                continue;
            };
            if !intersects || enter < 0.0 || enter >= trace.fraction || enter > leave {
                continue;
            }

            trace.fraction = enter;
            trace.end_position = std::array::from_fn(|i| start[i] + enter * (end[i] - start[i]));
            trace.plane_normal = [0.0; 3];
            trace.plane_normal[axis] = hit_normal;
            trace.surface_flags = obstacle.surface_flags;
            trace.contents = CONTENTS_SOLID;
            trace.entity = obstacle.entity;
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
    geometry: Geometry,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Geometry {
    Flat,
    Slope,
    Step,
    Stairs,
    BlockedStand,
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
        Geometry::Flat,
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
        Geometry::Flat,
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
        Geometry::Flat,
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
        Geometry::Flat,
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
        Geometry::Flat,
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
        Geometry::Flat,
    ),
    case(
        "land_medium",
        0.0,
        180.0,
        220.0,
        -300.0,
        false,
        false,
        false,
        DEFAULT_TOLERANCE,
        Geometry::Flat,
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
        Geometry::Flat,
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
        Geometry::Flat,
    ),
    case(
        "slope_walk",
        0.0,
        24.0,
        0.0,
        0.0,
        false,
        false,
        false,
        DEFAULT_TOLERANCE,
        Geometry::Slope,
    ),
    case(
        "step_up",
        0.0,
        24.0,
        0.0,
        0.0,
        false,
        false,
        false,
        DEFAULT_TOLERANCE,
        Geometry::Step,
    ),
    case(
        "stairs_walk",
        0.0,
        24.0,
        0.0,
        0.0,
        false,
        false,
        false,
        DEFAULT_TOLERANCE,
        Geometry::Stairs,
    ),
    case(
        "crouch_slide",
        0.0,
        120.0,
        450.0,
        -200.0,
        false,
        false,
        false,
        DEFAULT_TOLERANCE,
        Geometry::Flat,
    ),
    case(
        "crouch_uncrouch",
        0.0,
        24.0,
        0.0,
        0.0,
        false,
        false,
        false,
        DEFAULT_TOLERANCE,
        Geometry::Flat,
    ),
    case(
        "crouch_blocked_stand",
        0.0,
        24.0,
        0.0,
        0.0,
        false,
        false,
        false,
        DEFAULT_TOLERANCE,
        Geometry::BlockedStand,
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
        Geometry::Flat,
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
    geometry: Geometry,
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
        geometry,
    }
}

struct Row {
    scenario: String,
    rate: f32,
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
    view_height: f32,
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
            Row {
                scenario: f[0].to_string(),
                rate: f[1].parse::<f32>().unwrap(),
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
                view_height: float(25),
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
    let mut planes = if case.geometry == Geometry::Slope {
        vec![CollisionPlane {
            normal: [-0.24253562, 0.0, 0.9701425],
            distance: 0.0,
            surface_flags: 0,
            entity: 1,
        }]
    } else {
        vec![CollisionPlane {
            normal: [0.0, 0.0, 1.0],
            distance: 0.0,
            surface_flags: 0,
            entity: 1,
        }]
    };
    if case.wall {
        planes.push(CollisionPlane {
            normal: [-1.0, 0.0, 0.0],
            distance: -110.0,
            surface_flags: if case.ladder { SURF_LADDER } else { 0 },
            entity: 2,
        });
    }
    let boxes = match case.geometry {
        Geometry::Step => vec![CollisionBox {
            mins: [48.0, -128.0, 0.0],
            maxs: [80.0, 128.0, 12.0],
            surface_flags: 0,
            entity: 3,
        }],
        Geometry::Stairs => vec![
            CollisionBox {
                mins: [48.0, -128.0, 0.0],
                maxs: [72.0, 128.0, 6.0],
                surface_flags: 0,
                entity: 3,
            },
            CollisionBox {
                mins: [72.0, -128.0, 0.0],
                maxs: [96.0, 128.0, 12.0],
                surface_flags: 0,
                entity: 4,
            },
            CollisionBox {
                mins: [96.0, -128.0, 0.0],
                maxs: [120.0, 128.0, 18.0],
                surface_flags: 0,
                entity: 5,
            },
        ],
        Geometry::BlockedStand => vec![CollisionBox {
            mins: [-32.0, -64.0, 40.0],
            maxs: [48.0, 64.0, 80.0],
            surface_flags: 0,
            entity: 3,
        }],
        Geometry::Flat | Geometry::Slope => Vec::new(),
    };
    HarnessWorld {
        planes,
        boxes,
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
        let view_height_error = (state.view_height - row.view_height).abs();
        if view_height_error > case.tolerance {
            report.tolerance_mismatches.push(format!(
                "{at}: view height {} vs {}",
                state.view_height, row.view_height
            ));
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

fn frame_msec(rate: f32) -> u8 {
    match rate {
        125.0 => 8,
        100.0 => 10,
        62.5 => 16,
        60.0 => 17,
        50.0 => 20,
        30.0 => 33,
        _ => panic!("unsupported fixture rate {rate}"),
    }
}

fn run_case(name: &str) {
    let case = CASES.iter().find(|c| c.name == name).unwrap();
    let all = rows();
    for &rate in RATES {
        let rows: Vec<&Row> = all
            .iter()
            .filter(|r| r.scenario == name && r.rate == rate)
            .collect();
        let msec = frame_msec(rate);
        assert_eq!(
            rows.len(),
            1600_usize.div_ceil(msec as usize),
            "{name} at {rate} Hz"
        );
        for (index, row) in rows.iter().enumerate() {
            assert_eq!(row.step, index, "{name} at {rate} Hz");
            assert_eq!(row.command.msec, msec, "{name} at {rate} Hz");
        }

        let report = replay(case, &rows);
        println!(
            "{name} at {rate} Hz: {} steps, exact mismatches {}, tolerance mismatches {}, max |position error| {:.4}, max |velocity error| {:.4}",
            report.steps,
            report.exact_mismatches.len(),
            report.tolerance_mismatches.len(),
            report.max_position_error,
            report.max_velocity_error
        );
        assert!(
            report.exact_mismatches.is_empty(),
            "exact invariant mismatches (movement integration) at {rate} Hz:\n{}",
            report.exact_mismatches[..report.exact_mismatches.len().min(10)].join("\n")
        );
        assert!(
            report.tolerance_mismatches.is_empty(),
            "trajectory outside tolerance {} (movement integration) at {rate} Hz:\n{}",
            case.tolerance,
            report.tolerance_mismatches[..report.tolerance_mismatches.len().min(10)].join("\n")
        );
    }
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
    land_medium => "land_medium",
    land_fast => "land_fast",
    water_swim => "water_swim",
    slope_walk => "slope_walk",
    step_up => "step_up",
    stairs_walk => "stairs_walk",
    crouch_slide => "crouch_slide",
    crouch_uncrouch => "crouch_uncrouch",
    crouch_blocked_stand => "crouch_blocked_stand",
    ladder_climb => "ladder_climb",
}

#[test]
fn fixture_covers_every_case() {
    let all = rows();
    for case in CASES {
        for &rate in RATES {
            assert!(
                all.iter()
                    .any(|row| row.scenario == case.name && row.rate == rate),
                "{} at {} Hz",
                case.name,
                rate
            );
        }
    }
    let rows_per_rate = RATES
        .iter()
        .map(|&rate| 1600_usize.div_ceil(frame_msec(rate) as usize))
        .sum::<usize>();
    assert_eq!(all.len(), CASES.len() * rows_per_rate);
}
