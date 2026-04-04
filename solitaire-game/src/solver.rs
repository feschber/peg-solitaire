use futures_lite::future::{self, block_on};
use solitaire_solver::{HashMap, HashSet, SolutionMultiset};

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
        app.add_systems(
            Update,
            calculate_unique_solutions.run_if(resource_added::<FeasibleConstellations>),
        );
        app.add_systems(
            Update,
            calculate_unique_paths.run_if(resource_added::<FeasibleConstellations>),
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
pub struct UniquePaths(pub HashMap<Board, u64>);

#[derive(Component)]
struct BackgroundTask {
    task: Task<CommandQueue>,
}

fn create_solution_dag(mut commands: Commands, wake: Res<EventLoopProxyWrapper>) {
    info!("calculating feasible constellations ...");
    let thread_pool = AsyncComputeTaskPool::get();
    let entity = commands.spawn_empty().id();
    let wake = wake.clone();
    let task = thread_pool.spawn(async move {
        let feasible = solitaire_solver::calculate_all_solutions(None);

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
        let feasible = feasible.iter().copied().collect();
        let p_random_chance = solitaire_solver::calculate_p_random_chance_success(feasible);

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
        let unique_paths = solitaire_solver::all_unique_paths(feasible.iter().copied());
        info!("unique solutions: {}", unique_paths.len());

        let mut command_queue = CommandQueue::default();
        command_queue.push(move |world: &mut World| {
            world.insert_resource(UniquePaths(unique_paths));
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
