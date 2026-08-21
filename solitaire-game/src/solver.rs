use futures_lite::future::{self, block_on};
use solitaire_solver::dominators::{ForcedJump, forced_jumps};
use solitaire_solver::{HashMap, HashSet, SolutionMultiset};

use crate::CurrentBoard;
use bevy::{
    ecs::world::CommandQueue,
    prelude::*,
    tasks::{AsyncComputeTaskPool, Task},
    window::RequestRedraw,
    winit::{EventLoopProxyWrapper, WinitUserEvent::WakeUp},
};
use solitaire_solver::Board;

pub struct Solver;

impl Plugin for Solver {
    fn build(&self, app: &mut bevy::app::App) {
        app.add_systems(Startup, create_solution_dag);
        app.add_systems(
            Update,
            calculate_random_move_chances.run_if(resource_added::<FeasibleConstellations>),
        );
        #[cfg(not(target_arch = "wasm32"))]
        app.add_systems(
            Update,
            calculate_unique_solutions.run_if(resource_added::<FeasibleConstellations>),
        );
        app.add_systems(
            Update,
            calculate_unique_paths.run_if(resource_added::<FeasibleConstellations>),
        );
        app.add_systems(
            Update,
            schedule_forced_jumps.run_if(
                resource_exists::<UniquePaths>.and_then(not(resource_exists::<ForcedJumpsPending>)),
            ),
        );
        app.add_systems(Update, poll_task);
    }
}

#[derive(Resource)]
pub struct FeasibleConstellations(pub HashSet<Board>);

#[derive(Resource)]
pub struct RandomMoveChances(pub HashMap<Board, f64>);

#[derive(Resource)]
pub struct UniqueSolutions(pub Vec<SolutionMultiset>);

#[derive(Resource)]
pub struct UniquePaths(pub std::sync::Arc<HashMap<Board, u64>>);

/// The jumps every winning continuation from [`board`](Self::board) still has to make.
///
/// Carries the board it was computed for because it is computed off-thread and the player can
/// move meanwhile: a result for a position that has since been left is worse than none, so
/// consumers compare against [`crate::CurrentBoard`] before trusting it.
#[derive(Resource)]
pub struct ForcedJumps {
    pub board: Board,
    pub jumps: Vec<ForcedJump>,
}

/// Present while a [`ForcedJumps`] computation is in flight, so moves made during a long one
/// queue up as "recompute when it lands" rather than stacking a task per move.
#[derive(Resource)]
struct ForcedJumpsPending;

/// A unit of work running on the async pool, polled by [`poll_task`].
///
/// Shared with `graph.rs` so its build stage gets polled by the same system.
#[derive(Component)]
pub struct BackgroundTask {
    pub task: Task<CommandQueue>,
}

fn create_solution_dag(mut commands: Commands, wake: Res<EventLoopProxyWrapper>) {
    info!("calculating feasible constellations ...");
    let thread_pool = AsyncComputeTaskPool::get();
    let entity = commands.spawn_empty().id();
    let wake = wake.clone();
    let task = thread_pool.spawn(async move {
        let feasible = solitaire_solver::calculate_feasible_set(None);

        let feasible_hashset = HashSet::from_iter(feasible.iter().copied());
        let mut command_queue = CommandQueue::default();
        command_queue.push(move |world: &mut World| {
            info!("feasible constellations calculated!");
            world.insert_resource(FeasibleConstellations(feasible_hashset));
            world.entity_mut(entity).remove::<BackgroundTask>();
        });
        wake.send_event(WakeUp).unwrap();
        command_queue
    });
    commands.entity(entity).insert(BackgroundTask { task });
}

fn calculate_random_move_chances(
    mut commands: Commands,
    feasible: Res<FeasibleConstellations>,
    wake: Res<EventLoopProxyWrapper>,
) {
    info!("calculating P(\"success by random moves\") ...");
    let thread_pool = AsyncComputeTaskPool::get();
    let entity = commands.spawn_empty().id();
    let feasible = feasible.0.clone();
    let wake = wake.clone();
    let task = thread_pool.spawn(async move {
        let feasible = feasible;
        let p_random_chance =
            solitaire_solver::calculate_p_random_chance_success(feasible.into_iter());

        let mut command_queue = CommandQueue::default();
        command_queue.push(move |world: &mut World| {
            world.insert_resource(RandomMoveChances(p_random_chance));
            world.entity_mut(entity).remove::<BackgroundTask>();
        });
        wake.send_event(WakeUp).unwrap();
        command_queue
    });
    commands.entity(entity).insert(BackgroundTask { task });
}

fn calculate_unique_solutions(
    mut commands: Commands,
    feasible: Res<FeasibleConstellations>,
    wake: Res<EventLoopProxyWrapper>,
) {
    info!("calculating unique solutions ...");
    let thread_pool = AsyncComputeTaskPool::get();
    let entity = commands.spawn_empty().id();
    let feasible = feasible.0.clone();
    let wake = wake.clone();
    let task = thread_pool.spawn(async move {
        let unique_solutions =
            solitaire_solver::all_unique_solutions(Board::default(), feasible.iter().copied());
        info!("unique solutions: {}", unique_solutions.len());

        let mut command_queue = CommandQueue::default();
        command_queue.push(move |world: &mut World| {
            world.insert_resource(UniqueSolutions(unique_solutions.into_iter().collect()));
            world.entity_mut(entity).remove::<BackgroundTask>();
        });
        wake.send_event(WakeUp).unwrap();
        command_queue
    });
    commands.entity(entity).insert(BackgroundTask { task });
}

fn calculate_unique_paths(
    mut commands: Commands,
    feasible: Res<FeasibleConstellations>,
    wake: Res<EventLoopProxyWrapper>,
) {
    info!("calculating unique paths ...");
    let thread_pool = AsyncComputeTaskPool::get();
    let entity = commands.spawn_empty().id();
    let feasible = feasible.0.clone();
    let wake = wake.clone();
    let task = thread_pool.spawn(async move {
        // `None` = all cores: this already runs on the async pool, off the main thread,
        // so there is no frame budget to protect here
        let unique_paths = solitaire_solver::all_unique_paths(feasible.iter().copied(), None);
        info!("unique solutions: {}", unique_paths.len());

        let mut command_queue = CommandQueue::default();
        command_queue.push(move |world: &mut World| {
            world.insert_resource(UniquePaths(std::sync::Arc::new(unique_paths)));
            world.entity_mut(entity).remove::<BackgroundTask>();
        });
        wake.send_event(WakeUp).unwrap();
        command_queue
    });
    commands.entity(entity).insert(BackgroundTask { task });
}

/// Recomputes [`ForcedJumps`] whenever it is missing or stale for the current board.
///
/// Not cheap early on - the traversal covers every still-winnable board reachable from the
/// current one, which near the opening is most of the game: measured at ~16 s and ~190 MB of
/// transient set at 32 pegs, falling to ~460 ms by 26 pegs and under 10 ms by 24. That is why
/// it runs on the async pool and why the result is stamped with its board instead of assumed
/// current. It is also why it is worth doing at all rather than per-frame: the answer only
/// changes when the board does.
///
/// The opening is also where it has nothing to report - the earliest position with a forced
/// jump over 60 sampled winning lines was 27 pegs - so the expensive end of the range is the
/// end that returns empty. Left uncapped anyway: a peg-count cutoff would be a silent claim
/// that nothing is forced above it, which the sampling does not establish.
fn schedule_forced_jumps(
    mut commands: Commands,
    board: Res<CurrentBoard>,
    paths: Res<UniquePaths>,
    forced: Option<Res<ForcedJumps>>,
    wake: Res<EventLoopProxyWrapper>,
) {
    if forced.is_some_and(|forced| forced.board == board.0) {
        return;
    }
    let thread_pool = AsyncComputeTaskPool::get();
    let entity = commands.spawn_empty().id();
    let target = board.0;
    // an `Arc` clone: this runs on every move, and the counts map is one entry per
    // feasible board - copying it per move would dwarf the traversal it feeds
    let counts = paths.0.clone();
    let wake = wake.clone();
    commands.insert_resource(ForcedJumpsPending);
    let task = thread_pool.spawn(async move {
        let jumps = forced_jumps(target, &counts);

        let mut command_queue = CommandQueue::default();
        command_queue.push(move |world: &mut World| {
            world.insert_resource(ForcedJumps {
                board: target,
                jumps,
            });
            world.remove_resource::<ForcedJumpsPending>();
            world.entity_mut(entity).remove::<BackgroundTask>();
        });
        wake.send_event(WakeUp).unwrap();
        command_queue
    });
    commands.entity(entity).insert(BackgroundTask { task });
}

fn poll_task(
    mut commands: Commands,
    tasks: Query<(Entity, &mut BackgroundTask)>,
    mut request_redraw: MessageWriter<RequestRedraw>,
) {
    for (entity, mut task) in tasks {
        if let Some(mut commands_queue) = block_on(future::poll_once(&mut task.task)) {
            commands.append(&mut commands_queue);
            commands.entity(entity).despawn();
            request_redraw.write(RequestRedraw);
        }
    }
}
