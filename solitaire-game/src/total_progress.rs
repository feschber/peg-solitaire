use bevy::{
    app::Plugin,
    ecs::{
        observer::On,
        resource::Resource,
        system::{Res, ResMut},
    },
};
use solitaire_solver::{Board, HashSet, Solution};

use crate::{CurrentBoard, MoveEvent};

/// This module keeps track of the total progress of the game.
/// We store statistics about which constellations have previously been
/// explored.

pub struct TotalProgressPlugin;

#[derive(Default, Resource)]
pub struct TotalProgress {
    /// all states that have ever been seen
    pub explored_states: HashSet<Board>,
    /// all solutions that have ever been found
    pub found_solutions: HashSet<Solution>,
}

impl Plugin for TotalProgressPlugin {
    fn build(&self, app: &mut bevy::app::App) {
        app.init_resource::<TotalProgress>();
        app.add_observer(update_total_progress);
    }
}

fn update_total_progress(
    _: On<MoveEvent>,
    mut total_progress: ResMut<TotalProgress>,
    board: Res<CurrentBoard>,
) {
    total_progress.explored_states.insert(board.0);
}
