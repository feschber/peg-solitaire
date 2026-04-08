use bevy::{
    input::common_conditions::{input_just_pressed, input_just_released},
    prelude::*,
    window::PrimaryWindow,
};

use crate::{
    Selected,
    board::{BoardPosition, Peg},
    viewport_to_world,
};

/// triggers peg movement request events based on mouse / touch input
pub struct Input;

impl Plugin for Input {
    fn build(&self, app: &mut App) {
        app.add_systems(
            PreUpdate,
            grab_peg.run_if(input_just_pressed(MouseButton::Left)),
        );
        app.add_systems(
            PreUpdate,
            release_peg.run_if(input_just_released(MouseButton::Left)),
        );
        app.add_systems(PreUpdate, peg_selection_touch);
    }
}

#[derive(Event)]
pub struct RequestPegMove {
    pub src: BoardPosition,
    pub dst: BoardPosition,
}

fn grab_peg(
    mut commands: Commands,
    window: Single<&Window, With<PrimaryWindow>>,
    camera_query: Single<(&Camera, &GlobalTransform)>,
    pegs: Query<(Entity, &BoardPosition), With<Peg>>,
) {
    let (camera, camera_transform) = *camera_query;
    if let Some(cursor_pos) = window.cursor_position() {
        if let Some(world_pos_cursor) = viewport_to_world(cursor_pos, camera, camera_transform) {
            let board_pos = BoardPosition::from_world_space(world_pos_cursor.xy());
            if let Some(peg) = pegs.iter().find(|(_, p)| **p == board_pos).map(|(p, _)| p) {
                commands.entity(peg).insert(Selected);
            }
        };
    }
}

fn release_peg(
    mut commands: Commands,
    window: Single<&Window, With<PrimaryWindow>>,
    camera_query: Single<(&Camera, &GlobalTransform)>,
    selected_pegs: Query<(Entity, &BoardPosition), (With<Peg>, With<Selected>)>,
) {
    let (camera, camera_transform) = *camera_query;
    if let Some(cursor_pos) = window.cursor_position() {
        if let Some(world_pos_cursor) = viewport_to_world(cursor_pos, camera, camera_transform) {
            let board_pos = BoardPosition::from_world_space(world_pos_cursor.xy());
            for (selected_peg, &current_pos) in selected_pegs {
                move_peg(&mut commands, selected_peg, current_pos, board_pos);
            }
        };
    }
}

fn peg_selection_touch(
    mut commands: Commands,
    touches: Res<Touches>,
    camera_query: Single<(&Camera, &GlobalTransform)>,
    pegs: Query<(Entity, &BoardPosition), With<Peg>>,
    selected_pegs: Query<(Entity, &BoardPosition), (With<Peg>, With<Selected>)>,
) {
    let (camera, camera_transform) = *camera_query;
    for touch in touches.iter_just_pressed() {
        if let Some(world_pos) = viewport_to_world(touch.position(), camera, camera_transform) {
            let board_pos = BoardPosition::from_world_space(world_pos.xy());
            if let Some(peg) = pegs.iter().find(|(_, p)| **p == board_pos).map(|(p, _)| p) {
                commands.entity(peg).insert(Selected);
            }
        }
    }
    for touch in touches.iter_just_released() {
        if let Some(world_pos) = viewport_to_world(touch.position(), camera, camera_transform) {
            let board_pos = BoardPosition::from_world_space(world_pos.xy());
            for (selected_peg, &current_pos) in selected_pegs {
                move_peg(&mut commands, selected_peg, current_pos, board_pos);
            }
        }
    }
}

fn move_peg(commands: &mut Commands, selected: Entity, src: BoardPosition, dst: BoardPosition) {
    let diff = dst - src;
    let diff = BoardPosition {
        x: if diff.x == 0 { 0 } else { diff.x.signum() },
        y: if diff.y == 0 { 0 } else { diff.y.signum() },
    };
    if diff.x > diff.y {
        commands.trigger(RequestPegMove {
            src,
            dst: src + diff * 2,
        });
    } else if diff.y > diff.x {
        commands.trigger(RequestPegMove {
            src: src,
            dst: src + diff * 2,
        });
    }
    commands.entity(selected).remove::<Selected>();
}
