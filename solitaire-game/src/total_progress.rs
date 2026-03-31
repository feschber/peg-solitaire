use std::collections::HashSet;

use bevy::{
    app::Plugin,
    ecs::{
        observer::On,
        resource::Resource,
        schedule::common_conditions::resource_changed,
        system::{Commands, Res, ResMut},
    },
    prelude::*,
};
use solitaire_solver::{Board, HashMap, Solution};

use crate::{
    CurrentBoard, CurrentSolution, MoveEvent, SolutionEvent,
    solver::{FeasibleConstellations, UniqueSolutions},
    stats::UpdateStats,
};

/// This module keeps track of the total progress of the game.
/// We store statistics about which constellations have previously been
/// explored.

pub struct TotalProgressPlugin;

#[derive(Resource)]
pub struct TotalProgress {
    /// all states that have ever been seen (->amount)
    pub explored_states: HashMap<Board, usize>,
    /// all states that have ever been seen but normalized (->amount)
    pub normalized_explored_states: HashMap<Board, usize>,
    /// explored states by number of pegs
    pub explored_states_by_pegs: [HashSet<Board>; Board::SLOTS - 1],
    /// all unique solutions that have been explored
    pub unique_solutions: HashSet<Solution>,
    /// number of times the boared has been solved
    pub num_solutions: u64,
}

impl Default for TotalProgress {
    fn default() -> Self {
        Self {
            explored_states: HashMap::from_iter([(Board::default(), 1)]),
            normalized_explored_states: HashMap::from_iter([(Board::default(), 1)]),
            explored_states_by_pegs: Default::default(),
            unique_solutions: Default::default(),
            num_solutions: Default::default(),
        }
    }
}

impl Plugin for TotalProgressPlugin {
    fn build(&self, app: &mut bevy::app::App) {
        app.init_resource::<PossibleUniqueSolutions>();
        app.init_resource::<TotalProgress>();
        app.add_observer(update_total_progress);
        app.add_observer(update_solutions);
        app.add_systems(
            Update,
            update_unique_solutions.run_if(resource_changed::<CurrentSolution>),
        );
    }
}

fn update_total_progress(
    _: On<MoveEvent>,
    mut total_progress: ResMut<TotalProgress>,
    feasible: Option<Res<FeasibleConstellations>>,
    board: Res<CurrentBoard>,
) {
    let board = board.0;
    if let Some(feasible) = feasible {
        if feasible.0.contains(&board.normalize()) {
            *total_progress
                .explored_states
                .entry(board)
                .or_insert(Default::default()) += 1;
            *total_progress
                .normalized_explored_states
                .entry(board.normalize())
                .or_insert(Default::default()) += 1;
            total_progress.explored_states_by_pegs[board.count_balls() as usize - 1].insert(board);
        }
    }
}

#[derive(Default, Resource)]
pub struct PossibleUniqueSolutions(pub Option<usize>);

fn update_unique_solutions(
    current_solution: Res<CurrentSolution>,
    unique_solutions: Option<Res<UniqueSolutions>>,
    mut commands: Commands,
    mut possible_unique_solutions: ResMut<PossibleUniqueSolutions>,
) {
    if let Some(unique_solutions) = unique_solutions {
        let mut unique_solutions = unique_solutions.0.clone();
        for m in &current_solution.1 {
            unique_solutions.retain_mut(|e| {
                if let Some(count) = e.get_mut(&m.mov) {
                    if *count > 0 {
                        *count -= 1;
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            });
        }
        possible_unique_solutions.0.replace(unique_solutions.len());
        commands.trigger(UpdateStats);
    }
}

fn update_solutions(
    solution: On<SolutionEvent>,
    mut total_progress: ResMut<TotalProgress>,
    mut commands: Commands,
) {
    total_progress.unique_solutions.insert(solution.0.clone());
    total_progress.num_solutions += 1;
    commands.trigger(UpdateStats);
}
