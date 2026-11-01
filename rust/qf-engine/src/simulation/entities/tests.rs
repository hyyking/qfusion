use super::*;
use crate::simulation::pmove::{PmTrace, PmoveCommand, PmoveRules, PmoveWorld, pmove_with_touch};

/// Axis-aligned boxes standing in for inline brush models.
struct Boxes(Vec<(Vec3, Vec3)>);

impl BrushModels for Boxes {
    fn model_bounds(&self, index: usize) -> Option<(Vec3, Vec3)> {
        self.0.get(index).copied()
    }

    fn box_touches_model(&self, index: usize, origin: Vec3, mins: Vec3, maxs: Vec3) -> bool {
        let (lo, hi) = self.0[index];
        (0..3).all(|axis| {
            mins[axis] <= hi[axis] + origin[axis] && maxs[axis] >= lo[axis] + origin[axis]
        })
    }
}

struct EmptyWorld;

impl PmoveWorld for EmptyWorld {
    fn trace(&self, _: Vec3, _: Vec3, _: Vec3, end: Vec3, _: u32) -> PmTrace {
        PmTrace::miss(end)
    }

    fn point_contents(&self, _: Vec3) -> u32 {
        0
    }
}

fn ent(properties: &[(&str, &str)]) -> Entity {
    Entity {
        properties: properties
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect(),
    }
}

fn step(
    entities: &mut MapEntities,
    boxes: &Boxes,
    state: &mut PmoveState,
    time_ms: i64,
) -> Vec<PredictedEvent> {
    let command = PmoveCommand {
        msec: 8,
        ..PmoveCommand::default()
    };
    let mut touch = entities.player_touch(boxes, time_ms);
    pmove_with_touch(
        &EmptyWorld,
        &PmoveRules::default(),
        state,
        &command,
        &mut touch,
    )
    .events
}

#[test]
fn jump_pad_velocity_matches_legacy_formula() {
    // Pad centre (0,0,0), target 300 units east and 128 up, gravity 800.
    let boxes = Boxes(vec![
        ([0.0; 3], [0.0; 3]),
        ([-32.0, -32.0, -8.0], [32.0, 32.0, 8.0]),
    ]);
    let entities = [
        ent(&[("classname", "worldspawn"), ("gravity", "800")]),
        ent(&[
            ("classname", "trigger_push"),
            ("model", "*1"),
            ("target", "t1"),
        ]),
        ent(&[
            ("classname", "target_position"),
            ("targetname", "t1"),
            ("origin", "300 0 128"),
        ]),
    ];
    let map = MapEntities::spawn(&entities, &boxes);
    let velocity = map.push_velocity(1).unwrap();
    let time = (128.0f64 / 400.0).sqrt() as f32;
    assert!((velocity[0] - 300.0 / time).abs() < 1e-3);
    assert_eq!(velocity[1], 0.0);
    assert!((velocity[2] - time * 800.0).abs() < 1e-3);
    // Ballistic check: apex height at that airtime equals the target height.
    assert!((velocity[2] * time - 0.5 * 800.0 * time * time - 128.0).abs() < 0.01);
}

#[test]
fn jump_pad_without_target_uses_angle_and_speed() {
    let boxes = Boxes(vec![([0.0; 3], [0.0; 3]), ([-8.0; 3], [8.0; 3])]);
    let entities = [
        ent(&[
            ("classname", "trigger_push"),
            ("model", "*1"),
            ("angle", "-1"),
        ]),
        ent(&[
            ("classname", "trigger_push"),
            ("model", "*1"),
            ("angle", "90"),
            ("speed", "50"),
        ]),
    ];
    let map = MapEntities::spawn(&entities, &boxes);
    assert_eq!(map.push_velocity(0), Some([0.0, 0.0, 10_000.0]));
    let side = map.push_velocity(1).unwrap();
    assert!(side[0].abs() < 1e-3 && (side[1] - 500.0).abs() < 1e-3);
}

#[test]
fn jump_pad_not_below_target_is_removed_with_warning() {
    let boxes = Boxes(vec![([0.0; 3], [0.0; 3]), ([-8.0; 3], [8.0; 3])]);
    let entities = [
        ent(&[
            ("classname", "trigger_push"),
            ("model", "*1"),
            ("target", "t"),
        ]),
        ent(&[
            ("classname", "info_notnull"),
            ("targetname", "t"),
            ("origin", "9 0 0"),
        ]),
    ];
    let map = MapEntities::spawn(&entities, &boxes);
    assert_eq!(map.trigger_count(), 0);
    assert_eq!(map.warnings().len(), 1);
}

#[test]
fn player_on_jump_pad_is_launched_deterministically() {
    let boxes = Boxes(vec![
        ([0.0; 3], [0.0; 3]),
        ([-64.0, -64.0, -8.0], [64.0, 64.0, 8.0]),
    ]);
    let entities = [
        ent(&[("classname", "worldspawn"), ("gravity", "800")]),
        ent(&[
            ("classname", "trigger_push"),
            ("model", "*1"),
            ("target", "t1"),
        ]),
        ent(&[
            ("classname", "target_position"),
            ("targetname", "t1"),
            ("origin", "300 0 128"),
        ]),
    ];
    let expected = MapEntities::spawn(&entities, &boxes)
        .push_velocity(1)
        .unwrap();
    let run = || {
        let mut map = MapEntities::spawn(&entities, &boxes);
        let mut state = PmoveState::new([0.0, 0.0, 24.0]);
        state.flags |= PMF_ON_GROUND;
        let events = step(&mut map, &boxes, &mut state, 8);
        (state, events, map.drain_events())
    };
    let (state, events, triggered) = run();
    assert_eq!(state.velocity, expected);
    assert!(state.flags & PMF_ON_GROUND == 0);
    assert!(events.iter().any(|e| e.event == PmEvent::JumpPad));
    assert_eq!(
        triggered,
        vec![TriggerEvent::JumpPad {
            entity: 1,
            velocity: expected
        }]
    );
    let (again, _, _) = run();
    assert_eq!(again, state);
}

#[test]
fn jump_pad_rearms_after_wait_and_ignores_spectators() {
    let boxes = Boxes(vec![
        ([0.0; 3], [0.0; 3]),
        ([-64.0, -64.0, -8.0], [64.0, 64.0, 8.0]),
    ]);
    let entities = [ent(&[
        ("classname", "trigger_push"),
        ("model", "*1"),
        ("angle", "-1"),
    ])];
    let mut map = MapEntities::spawn(&entities, &boxes);
    let mut state = PmoveState::new([0.0, 0.0, 24.0]);
    step(&mut map, &boxes, &mut state, 8);
    assert_eq!(map.drain_events().len(), 1);
    // Within the 100 ms default wait: no second push.
    for tick in 2..=13 {
        state = PmoveState::new([0.0, 0.0, 24.0]);
        step(&mut map, &boxes, &mut state, tick * 8);
    }
    assert!(map.drain_events().is_empty());
    state = PmoveState::new([0.0, 0.0, 24.0]);
    step(&mut map, &boxes, &mut state, 14 * 8);
    assert_eq!(map.drain_events().len(), 1);

    let mut spectator = PmoveState::new([0.0, 0.0, 24.0]);
    spectator.pm_type = PmType::Spectator;
    step(&mut map, &boxes, &mut spectator, 40 * 8);
    assert!(map.drain_events().is_empty());
    assert_eq!(spectator.velocity, [0.0; 3]);
}

#[test]
fn push_once_removes_itself() {
    let boxes = Boxes(vec![
        ([0.0; 3], [0.0; 3]),
        ([-64.0, -64.0, -8.0], [64.0, 64.0, 8.0]),
    ]);
    let entities = [ent(&[
        ("classname", "trigger_push"),
        ("model", "*1"),
        ("angle", "-1"),
        ("spawnflags", "1"),
    ])];
    let mut map = MapEntities::spawn(&entities, &boxes);
    let mut state = PmoveState::new([0.0, 0.0, 24.0]);
    step(&mut map, &boxes, &mut state, 8);
    assert_eq!(map.trigger_count(), 0);
}

#[test]
fn teleporter_moves_player_keeping_horizontal_speed_along_destination() {
    let boxes = Boxes(vec![
        ([0.0; 3], [0.0; 3]),
        ([-64.0, -64.0, -8.0], [64.0, 64.0, 8.0]),
    ]);
    let entities = [
        ent(&[
            ("classname", "trigger_teleport"),
            ("model", "*1"),
            ("target", "d"),
        ]),
        ent(&[
            ("classname", "misc_teleporter_dest"),
            ("targetname", "d"),
            ("origin", "1000 2000 3000"),
            ("angle", "90"),
        ]),
    ];
    let mut map = MapEntities::spawn(&entities, &boxes);
    let mut state = PmoveState::new([0.0, 0.0, 24.0]);
    state.velocity = [300.0, 400.0, -700.0];
    let events = step(&mut map, &boxes, &mut state, 8);
    assert!(events.is_empty());
    assert_eq!(state.origin, [1000.0, 2000.0, 3000.0]);
    // 500 ups horizontal speed before the move, aimed along yaw 90.
    assert!(state.velocity[0].abs() < 1e-2);
    assert!((state.velocity[1] - 500.0).abs() < 1e-2);
    assert_eq!(state.velocity[2], 0.0);
    assert_eq!(state.view_angles, [0.0, 90.0, 0.0]);
    assert!(state.flags & PMF_TIME_TELEPORT != 0);
    assert_eq!(state.pm_time, 1);
    // Delta angles make the next command's view equal the destination view.
    assert_eq!(state.delta_angles, [0, 16384, 0]);
    assert!(matches!(
        map.drain_events().as_slice(),
        [TriggerEvent::Teleport { entity: 0, .. }]
    ));
}

#[test]
fn teleport_destination_info_teleport_destination_is_raised_16() {
    let boxes = Boxes(vec![
        ([0.0; 3], [0.0; 3]),
        ([-64.0, -64.0, -8.0], [64.0, 64.0, 8.0]),
    ]);
    let entities = [
        ent(&[
            ("classname", "trigger_teleport"),
            ("model", "*1"),
            ("target", "d"),
        ]),
        ent(&[
            ("classname", "info_teleport_destination"),
            ("targetname", "d"),
            ("origin", "0 0 100"),
        ]),
    ];
    let mut map = MapEntities::spawn(&entities, &boxes);
    let mut state = PmoveState::new([0.0, 0.0, 24.0]);
    step(&mut map, &boxes, &mut state, 8);
    assert_eq!(state.origin, [0.0, 0.0, 116.0]);
}

#[test]
fn teleporter_without_valid_target_is_not_spawned() {
    let boxes = Boxes(vec![([0.0; 3], [0.0; 3]), ([-8.0; 3], [8.0; 3])]);
    let entities = [
        ent(&[("classname", "trigger_teleport"), ("model", "*1")]),
        ent(&[
            ("classname", "trigger_teleport"),
            ("model", "*1"),
            ("target", "none"),
        ]),
    ];
    let map = MapEntities::spawn(&entities, &boxes);
    assert_eq!(map.trigger_count(), 0);
    assert_eq!(map.warnings().len(), 2);
}

#[test]
fn hurt_volume_reports_damage_every_wait_interval() {
    let boxes = Boxes(vec![
        ([0.0; 3], [0.0; 3]),
        ([-64.0, -64.0, -8.0], [64.0, 64.0, 8.0]),
    ]);
    let entities = [
        ent(&[
            ("classname", "trigger_hurt"),
            ("model", "*1"),
            ("dmg", "10"),
        ]),
        ent(&[
            ("classname", "trigger_hurt"),
            ("model", "*1"),
            ("dmg", "500"),
        ]),
    ];
    let mut map = MapEntities::spawn(&entities, &boxes);
    let mut state = PmoveState::new([0.0, 0.0, 24.0]);
    step(&mut map, &boxes, &mut state, 8);
    assert_eq!(
        map.drain_events(),
        vec![
            TriggerEvent::Hurt {
                entity: 0,
                damage: 10,
                kill: false,
                no_protection: false
            },
            TriggerEvent::Hurt {
                entity: 1,
                damage: 500,
                kill: true,
                no_protection: false
            },
        ]
    );
    // 100 ms wait: silent until time passes 8 + 100.
    step(&mut map, &boxes, &mut state, 16);
    assert!(map.drain_events().is_empty());
    step(&mut map, &boxes, &mut state, 116);
    assert_eq!(map.drain_events().len(), 2);
}

#[test]
fn swept_box_catches_fast_movement_through_a_thin_trigger() {
    // Legacy swept bounds: a trigger crossed within one tick is still touched.
    let boxes = Boxes(vec![
        ([0.0; 3], [0.0; 3]),
        ([90.0, -64.0, -64.0], [91.0, 64.0, 64.0]),
    ]);
    let entities = [ent(&[("classname", "trigger_hurt"), ("model", "*1")])];
    let mut map = MapEntities::spawn(&entities, &boxes);
    let mut state = PmoveState::new([0.0, 0.0, 0.0]);
    state.origin = [200.0, 0.0, 0.0];
    let context = TouchContext {
        previous_origin: [0.0, 0.0, 0.0],
        previous_velocity: [0.0; 3],
        command_angles: [0; 3],
    };
    let mut events = Vec::new();
    map.touch(&boxes, 8, &mut state, &context, &mut events);
    assert_eq!(map.drain_events().len(), 1);
}

#[test]
fn unsupported_classnames_are_counted() {
    let boxes = Boxes(vec![([0.0; 3], [0.0; 3])]);
    let entities = [
        ent(&[("classname", "worldspawn")]),
        ent(&[("classname", "item_health_small")]),
        ent(&[("classname", "ITEM_HEALTH_SMALL")]),
        ent(&[("classname", "target_position")]),
    ];
    let map = MapEntities::spawn(&entities, &boxes);
    assert_eq!(
        map.unsupported_classnames().get("item_health_small"),
        Some(&2)
    );
    assert_eq!(map.unsupported_classnames().len(), 1);
}

#[test]
fn atof_parses_prefixes() {
    assert_eq!(atof("0.2"), 0.2);
    assert_eq!(atof("-1"), -1.0);
    assert_eq!(atof("12abc"), 12.0);
    assert_eq!(atof("abc"), 0.0);
}

fn room() -> Boxes {
    Boxes(vec![
        ([0.0; 3], [0.0; 3]),
        ([-64.0, -64.0, -8.0], [64.0, 64.0, 8.0]),
    ])
}

fn used(source: usize, target: usize) -> TriggerEvent {
    TriggerEvent::Used { source, target }
}

#[test]
fn trigger_multiple_fires_chain_and_honours_wait() {
    let boxes = room();
    let entities = [
        ent(&[
            ("classname", "trigger_multiple"),
            ("model", "*1"),
            ("target", "a"),
            ("message", "hi"),
        ]),
        ent(&[
            ("classname", "target_relay"),
            ("targetname", "a"),
            ("target", "b"),
        ]),
        ent(&[("classname", "target_kill"), ("targetname", "b")]),
    ];
    let mut map = MapEntities::spawn(&entities, &boxes);
    let mut state = PmoveState::new([0.0, 0.0, 24.0]);
    step(&mut map, &boxes, &mut state, 8);
    assert_eq!(
        map.drain_events(),
        vec![
            TriggerEvent::Message {
                entity: 0,
                text: "hi".into()
            },
            used(0, 1),
            used(1, 2),
            TriggerEvent::Kill { entity: 2 },
        ]
    );
    // Default wait of 0.2 s.
    step(&mut map, &boxes, &mut state, 100);
    assert!(map.drain_events().is_empty());
    step(&mut map, &boxes, &mut state, 209);
    assert_eq!(map.drain_events().len(), 4);
}

#[test]
fn trigger_once_frees_itself_and_not_player_flag_blocks() {
    let boxes = room();
    let entities = [
        ent(&[
            ("classname", "trigger_once"),
            ("model", "*1"),
            ("target", "a"),
        ]),
        ent(&[("classname", "target_relay"), ("targetname", "a")]),
        ent(&[
            ("classname", "trigger_multiple"),
            ("model", "*1"),
            ("spawnflags", "2"),
            ("target", "a"),
        ]),
    ];
    let mut map = MapEntities::spawn(&entities, &boxes);
    let mut state = PmoveState::new([0.0, 0.0, 24.0]);
    step(&mut map, &boxes, &mut state, 8);
    assert_eq!(map.drain_events(), vec![used(0, 1)]);
    assert!(map.triggers_of("trigger_once").is_empty());
    step(&mut map, &boxes, &mut state, 5000);
    assert!(map.drain_events().is_empty());
}

#[test]
fn triggered_multiple_starts_disabled_until_used() {
    let boxes = room();
    let entities = [
        ent(&[
            ("classname", "trigger_multiple"),
            ("model", "*1"),
            ("spawnflags", "4"),
            ("targetname", "t"),
            ("target", "a"),
        ]),
        ent(&[("classname", "target_relay"), ("targetname", "a")]),
    ];
    let mut map = MapEntities::spawn(&entities, &boxes);
    let mut state = PmoveState::new([0.0, 0.0, 24.0]);
    step(&mut map, &boxes, &mut state, 8);
    assert!(map.drain_events().is_empty());
    assert!(map.activate(0, 16));
    step(&mut map, &boxes, &mut state, 24);
    assert_eq!(map.drain_events(), vec![used(0, 1)]);
}

#[test]
fn counter_fires_after_its_count_and_killtarget_removes_entities() {
    let boxes = room();
    let entities = [
        ent(&[
            ("classname", "trigger_counter"),
            ("count", "2"),
            ("target", "a"),
            ("killtarget", "gone"),
        ]),
        ent(&[
            ("classname", "target_relay"),
            ("targetname", "a"),
            ("message", "fired"),
        ]),
        ent(&[("classname", "target_relay"), ("targetname", "gone")]),
    ];
    let mut map = MapEntities::spawn(&entities, &boxes);
    assert!(map.activate(0, 8));
    assert_eq!(
        map.drain_events(),
        vec![TriggerEvent::Message {
            entity: 0,
            text: "1 more to go...".into()
        }]
    );
    assert!(map.activate(0, 16));
    assert_eq!(
        map.drain_events(),
        vec![
            TriggerEvent::Message {
                entity: 0,
                text: "Sequence completed!".into()
            },
            used(0, 1),
            TriggerEvent::Message {
                entity: 1,
                text: "fired".into()
            },
        ]
    );
    assert_eq!(map.triggers_of("target_relay"), vec![1]);
    // The counter freed itself (wait -1).
    assert!(!map.activate(0, 24));
}

#[test]
fn target_delay_and_delayed_use_fire_on_think() {
    let boxes = room();
    let entities = [
        ent(&[
            ("classname", "target_delay"),
            ("targetname", "d"),
            ("wait", "0.5"),
            ("target", "a"),
        ]),
        ent(&[
            ("classname", "target_relay"),
            ("targetname", "a"),
            ("delay", "0.25"),
            ("message", "x"),
        ]),
    ];
    let mut map = MapEntities::spawn(&entities, &boxes);
    assert!(map.activate(0, 0));
    map.think(499);
    assert!(map.drain_events().is_empty());
    map.think(500);
    assert_eq!(map.drain_events(), vec![used(0, 1)]);
    // The relay's own delay defers its message by another 250 ms.
    map.think(749);
    assert!(map.drain_events().is_empty());
    map.think(750);
    assert_eq!(
        map.drain_events(),
        vec![TriggerEvent::Message {
            entity: 1,
            text: "x".into()
        }]
    );
}

#[test]
fn trigger_always_fires_once_after_doubled_delay() {
    let boxes = room();
    let entities = [
        ent(&[("classname", "trigger_always"), ("target", "a")]),
        ent(&[("classname", "target_relay"), ("targetname", "a")]),
    ];
    let mut map = MapEntities::spawn(&entities, &boxes);
    map.think(299);
    assert!(map.drain_events().is_empty());
    // The think fires at 300 ms; `delay` is applied again, so targets run at 600 ms.
    map.think(300);
    assert!(map.drain_events().is_empty());
    map.think(600);
    assert_eq!(map.drain_events(), vec![used(0, 1)]);
    map.think(10_000);
    assert!(map.drain_events().is_empty());
}

#[test]
fn teleporting_telefrags_overlapping_occupants_only() {
    let boxes = room();
    let entities = [
        ent(&[
            ("classname", "trigger_teleport"),
            ("model", "*1"),
            ("target", "d"),
        ]),
        ent(&[
            ("classname", "misc_teleporter_dest"),
            ("targetname", "d"),
            ("origin", "1000 2000 3000"),
        ]),
    ];
    let mut map = MapEntities::spawn(&entities, &boxes);
    let occupant = |id, x| Occupant {
        id,
        origin: [x, 2000.0, 3000.0],
        mins: [-16.0, -16.0, -24.0],
        maxs: [16.0, 16.0, 40.0],
    };
    // 1 overlaps, 2 only touches faces with the player's box, 3 is far away.
    map.set_occupants(vec![
        occupant(1, 1010.0),
        occupant(2, 1032.0),
        occupant(3, 0.0),
    ]);
    let mut state = PmoveState::new([0.0, 0.0, 24.0]);
    step(&mut map, &boxes, &mut state, 8);
    let events = map.drain_events();
    assert!(matches!(
        events[0],
        TriggerEvent::Teleport { entity: 0, .. }
    ));
    assert_eq!(
        events[1..],
        [TriggerEvent::Telefrag {
            entity: 0,
            victim: Victim::Occupant(1)
        }]
    );
}

#[test]
fn target_teleporter_and_killbox_act_on_player_and_occupants() {
    let boxes = room();
    let entities = [
        ent(&[
            ("classname", "target_teleporter"),
            ("targetname", "tp"),
            ("target", "d"),
        ]),
        ent(&[
            ("classname", "target_position"),
            ("targetname", "d"),
            ("origin", "0 0 24"),
        ]),
        ent(&[
            ("classname", "func_killbox"),
            ("targetname", "kb"),
            ("model", "*1"),
        ]),
    ];
    let mut map = MapEntities::spawn(&entities, &boxes);
    map.set_occupants(vec![Occupant {
        id: 9,
        origin: [0.0, 0.0, 0.0],
        mins: [-16.0, -16.0, -24.0],
        maxs: [16.0, 16.0, 40.0],
    }]);

    // Without a player in the tick the teleporter has nobody to move.
    assert!(map.activate(0, 0));
    assert!(map.drain_events().is_empty());

    let mut state = PmoveState::new([500.0, 0.0, 24.0]);
    let context = TouchContext {
        previous_origin: state.origin,
        previous_velocity: [0.0; 3],
        command_angles: [0; 3],
    };
    // The killbox touch hook path: use it through a player tick via a relay-free use.
    let mut predicted = Vec::new();
    map.touch(&boxes, 8, &mut state, &context, &mut predicted);
    assert!(map.drain_events().is_empty());
    assert_eq!(map.triggers_of("func_killbox"), vec![2]);
    assert!(map.activate(2, 16));
    assert_eq!(
        map.drain_events(),
        vec![TriggerEvent::Telefrag {
            entity: 2,
            victim: Victim::Occupant(9)
        }]
    );
}

#[test]
fn gametype_filters_follow_g_can_spawn_entity() {
    let boxes = room();
    let entities = [
        ent(&[("classname", "worldspawn"), ("notteam", "1")]),
        ent(&[("classname", "target_relay"), ("notteam", "1")]),
        ent(&[("classname", "target_relay"), ("notfree", "1")]),
        ent(&[("classname", "target_relay"), ("gametype", "bomb, ctf")]),
        ent(&[("classname", "target_relay"), ("not_gametype", "BOMB")]),
        ent(&[("classname", "target_print"), ("message", "hello")]),
    ];
    let count = |gametype: &Gametype| {
        let map = MapEntities::spawn_for_gametype(&entities, &boxes, gametype);
        map.triggers_of("target_relay")
    };
    assert_eq!(count(&Gametype::bomb()), vec![2, 3]);
    assert_eq!(count(&Gametype::free_for_all()), vec![1, 4]);
    let mut duel = Gametype::bomb();
    duel.max_players_in_team = 1;
    assert_eq!(count(&duel), vec![1, 3]);

    let mut map = MapEntities::spawn(&entities, &boxes);
    assert!(map.activate(5, 0));
    assert_eq!(
        map.drain_events(),
        vec![TriggerEvent::Message {
            entity: 5,
            text: "hello".into()
        }]
    );
}

#[test]
fn every_supported_classname_is_instantiated() {
    let boxes = room();
    for &classname in SUPPORTED_TRIGGERS
        .iter()
        .chain(SUPPORTED_ACTIONS)
        .chain(SUPPORTED_TARGETS)
    {
        // Volumes need a model, teleporters and pads need a target.
        let entities = [
            ent(&[
                ("classname", classname),
                ("model", "*1"),
                ("target", "t"),
                ("message", "m"),
            ]),
            ent(&[
                ("classname", "target_position"),
                ("targetname", "t"),
                ("origin", "0 0 500"),
            ]),
        ];
        let map = MapEntities::spawn(&entities, &boxes);
        assert!(
            map.warnings().is_empty(),
            "{classname}: {:?}",
            map.warnings()
        );
        assert!(
            !map.unsupported_classnames().contains_key(classname),
            "{classname}"
        );
        assert!(map.triggers_of(classname).contains(&0), "{classname}");
    }
}

#[test]
fn event_stream_is_deterministic() {
    let run = || {
        let boxes = room();
        let entities = [
            ent(&[
                ("classname", "trigger_multiple"),
                ("model", "*1"),
                ("target", "a"),
            ]),
            ent(&[
                ("classname", "target_relay"),
                ("targetname", "a"),
                ("delay", "0.016"),
                ("message", "m"),
            ]),
        ];
        let mut map = MapEntities::spawn(&entities, &boxes);
        let mut state = PmoveState::new([0.0, 0.0, 24.0]);
        let mut all = Vec::new();
        for tick in 1..100 {
            step(&mut map, &boxes, &mut state, tick * 8);
            all.extend(map.drain_events());
        }
        all
    };
    let first = run();
    assert!(!first.is_empty());
    assert_eq!(first, run());
}
