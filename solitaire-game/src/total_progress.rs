use std::collections::HashSet;

use bevy::{
    app::Plugin,
    ecs::{
        observer::On,
        resource::Resource,
        system::{Commands, Res, ResMut},
    },
};
use solitaire_solver::{Board, Solution};

use crate::{
    CurrentBoard, MoveEvent, SolutionEvent,
    solver::{FeasibleConstellations, UniqueSolutions},
    stats::UpdateStats,
};

/// This module keeps track of the total progress of the game.
/// We store statistics about which constellations have previously been
/// explored.

pub struct TotalProgressPlugin;

#[derive(Default, Resource)]
pub struct TotalProgress {
    /// all states that have ever been seen
    pub explored_states: HashSet<Board>,
    /// explored states by number of pegs
    pub explored_states_by_pegs: [HashSet<Board>; Board::SLOTS - 1],
    /// all unique solutions that have been explored
    pub unique_solutions: HashSet<Solution>,
    /// number of times the boared has been solved
    pub num_solutions: u64,
}

impl Plugin for TotalProgressPlugin {
    fn build(&self, app: &mut bevy::app::App) {
        app.init_resource::<TotalProgress>();
        app.add_observer(update_total_progress);
        app.add_observer(update_solutions);
        app.add_observer(update_unique_solutions);
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
            total_progress.explored_states.insert(board);
            total_progress.explored_states_by_pegs[board.count_balls() as usize - 1].insert(board);
        }
    }
}

fn update_unique_solutions(
    mov: On<MoveEvent>,
    mut unique_solutions: Option<ResMut<UniqueSolutions>>,
    feasible: Option<Res<FeasibleConstellations>>,
    board: Res<CurrentBoard>,
    mut commands: Commands,
) {
    if let Some(mut unique_solutions) = unique_solutions {
        let mut unique_solutions = unique_solutions.as_mut();
        unique_solutions.0.retain_mut(|e| {
            if let Some(count) = e.get_mut(&mov.event().mov) {
                let ret = *count > 0;
                *count -= 1;
                ret
            } else {
                false
            }
        });
        mov.event().mov;
    }
    commands.trigger(UpdateStats);
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
