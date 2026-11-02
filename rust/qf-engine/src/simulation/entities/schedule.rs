//! The fixed-tick schedule of the entity world.
//!
//! One [`Schedule`] with chained sets `Touch -> Movers -> Finish`, run by the
//! [`MapEntities`](super::MapEntities) entry points on a single thread. Which sets do
//! work in a given run is selected by [`Run`], so callers that still drive the player
//! movement themselves (it runs between the thinks and the touches) keep their order
//! until movement becomes a system of this schedule.
//!
//! Systems are exclusive (`&mut World`). Caller-owned data that cannot live in the
//! world (brush models, static collision) is lent through [`Borrowed`].

use std::ptr::NonNull;

use bevy_ecs::prelude::*;
use bevy_ecs::schedule::{IntoScheduleConfigs, Schedule, ScheduleLabel, SingleThreadedExecutor};

use crate::simulation::pmove::{PmoveState, PmoveWorld, PredictedEvent, Vec3};

use super::components::{Outbox, Player};
use super::movers::PushedPlayer;
use super::{BrushModels, logic, movers, players};

#[derive(ScheduleLabel, Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct MapTick;

#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
enum Phase {
    Movement,
    Weapons,
    Touch,
    Movers,
    Finish,
}

/// Work selected for the next run of the schedule.
#[derive(Resource, Default)]
pub(super) struct Run {
    pub players: bool,
    pub thinks: bool,
    pub touch: bool,
    pub movers: bool,
}

/// Inputs and outputs of the touch pass.
#[derive(Resource, Default)]
pub(super) struct TouchFrame {
    pub swept: (Vec3, Vec3),
}

/// Inputs and outputs of the mover pass.
#[derive(Resource, Default)]
pub(super) struct MoverFrame {
    /// The pushed player's state and ground entity, written back after the run.
    pub player: Option<(PmoveState, Option<i32>)>,
}

/// Caller-owned references with their lifetimes erased for the duration of one run.
///
/// Invariant: set by [`Lend::new`] and cleared by [`Lend::drop`] around a single
/// `run_schedule`, so a non-null pointer is only observable while the referents are
/// alive and borrowed. Systems read them on the calling thread only (single-threaded
/// executor).
#[derive(Resource, Default)]
pub(super) struct Borrowed {
    models: Option<NonNull<dyn BrushModels>>,
    world: Option<NonNull<dyn PmoveWorld>>,
}

// SAFETY: the pointers are only non-null inside `run`, which executes every system on
// the calling thread; outside it the resource holds no pointers.
unsafe impl Send for Borrowed {}
unsafe impl Sync for Borrowed {}

impl Borrowed {
    pub(super) fn models(&self) -> &dyn BrushModels {
        let ptr = self.models.expect("brush models lent for this run");
        // SAFETY: see the type invariant.
        unsafe { ptr.as_ref() }
    }

    pub(super) fn world(&self) -> &dyn PmoveWorld {
        let ptr = self.world.expect("collision world lent for this run");
        // SAFETY: see the type invariant.
        unsafe { ptr.as_ref() }
    }
}

/// Lends `models` (and `collision`) to the schedule until dropped.
struct Lend<'w>(&'w mut World);

impl<'w> Lend<'w> {
    fn new(
        world: &'w mut World,
        models: Option<&dyn BrushModels>,
        collision: Option<&dyn PmoveWorld>,
    ) -> Self {
        let mut borrowed = world.resource_mut::<Borrowed>();
        // SAFETY: only the trait object lifetime bound is erased; `Lend` clears the
        // pointers before the borrows end, including on unwind.
        borrowed.models = models.map(|m| unsafe {
            std::mem::transmute::<NonNull<dyn BrushModels + '_>, NonNull<dyn BrushModels>>(
                NonNull::from(m),
            )
        });
        borrowed.world = collision.map(|w| unsafe {
            std::mem::transmute::<NonNull<dyn PmoveWorld + '_>, NonNull<dyn PmoveWorld>>(
                NonNull::from(w),
            )
        });
        Self(world)
    }
}

impl Drop for Lend<'_> {
    fn drop(&mut self) {
        let mut borrowed = self.0.resource_mut::<Borrowed>();
        borrowed.models = None;
        borrowed.world = None;
    }
}

/// Runs the schedule once with the lent references.
pub(super) fn run(
    world: &mut World,
    models: Option<&dyn BrushModels>,
    collision: Option<&dyn PmoveWorld>,
) {
    let lend = Lend::new(world, models, collision);
    lend.0.run_schedule(MapTick);
}

pub(super) fn install(world: &mut World) {
    world.init_resource::<Run>();
    world.init_resource::<TouchFrame>();
    world.init_resource::<MoverFrame>();
    world.init_resource::<Borrowed>();
    world.init_resource::<players::Fault>();
    super::weapons::install(world);

    let mut schedule = Schedule::new(MapTick);
    schedule.set_executor(SingleThreadedExecutor::new());
    schedule.configure_sets(
        (
            Phase::Movement,
            Phase::Weapons,
            Phase::Touch,
            Phase::Movers,
            Phase::Finish,
        )
            .chain(),
    );
    schedule.add_systems((
        players::movement
            .run_if(|run: Res<Run>| run.players)
            .in_set(Phase::Movement),
        super::weapons::step
            .run_if(|run: Res<Run>| run.players)
            .in_set(Phase::Weapons),
        (
            thinks.run_if(|run: Res<Run>| run.thinks || run.touch),
            touch_pass.run_if(|run: Res<Run>| run.touch),
        )
            .chain()
            .in_set(Phase::Touch),
        movers_step
            .run_if(|run: Res<Run>| run.movers)
            .in_set(Phase::Movers),
        finish.in_set(Phase::Finish),
    ));
    world.add_schedule(schedule);
}

fn thinks(world: &mut World) {
    logic::run_thinks(world);
}

fn touch_pass(world: &mut World) {
    let swept = world.resource::<TouchFrame>().swept;
    world.resource_scope(|world, borrowed: Mut<Borrowed>| {
        logic::touch_pass(world, borrowed.models(), swept);
    });
}

fn movers_step(world: &mut World) {
    if world.resource::<players::Fault>().0.is_some() {
        return;
    }
    let time_ms = world.resource::<super::components::Level>().time_ms;
    // Only the lowest slot is carried, pushed and crushed by movers.
    let lead = if world.resource::<Run>().players {
        players::in_slot_order(world).first().map(|&(_, e)| e)
    } else {
        None
    };
    let mut frame = match lead {
        Some(entity) => world
            .get::<players::Walker>(entity)
            .map(|w| (w.0.state, w.0.ground_entity)),
        None => std::mem::take(&mut world.resource_mut::<MoverFrame>().player),
    };
    world.resource_scope(|world, borrowed: Mut<Borrowed>| {
        let player = frame.as_mut().map(|(state, ground)| PushedPlayer {
            state,
            ground_entity: *ground,
        });
        movers::step(world, borrowed.models(), borrowed.world(), time_ms, player);
    });
    match lead {
        Some(entity) => {
            if let (Some((state, _)), Some(mut walker)) =
                (frame, world.get_mut::<players::Walker>(entity))
            {
                walker.0.state = state;
            }
        }
        None => world.resource_mut::<MoverFrame>().player = frame,
    }
}

fn finish(world: &mut World) {
    *world.resource_mut::<Run>() = Run::default();
}

/// Moves queued predicted events out of the outbox.
pub(super) fn take_predicted(world: &mut World, predicted: &mut Vec<PredictedEvent>) {
    predicted.append(&mut world.resource_mut::<Outbox>().predicted);
}

/// Clears the local player after a pass.
pub(super) fn take_player(world: &mut World) -> Option<super::components::PlayerPresence> {
    world.resource_mut::<Player>().0.take()
}
