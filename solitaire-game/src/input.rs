use bevy::{input::common_conditions::input_just_pressed, prelude::*, window::PrimaryWindow};

use crate::{
    PegMoved, Selected,
    board::{BoardPosition, Peg},
    viewport_to_world,
};

/// triggers peg movement request events based on mouse / touch input
pub struct Input;

impl Plugin for Input {
    fn build(&self, app: &mut App) {
        app.add_systems(
            PreUpdate,
            peg_selection_cursor.run_if(input_just_pressed(MouseButton::Left)),
        );
        app.add_systems(PreUpdate, peg_selection_touch);
        app.add_observer(on_board_clicked);
    }
}

#[derive(Event)]
pub struct RequestPegMove {
    pub src: BoardPosition,
    pub dst: BoardPosition,
}

#[derive(Resource)]
struct SelectedPos(BoardPosition);

#[derive(Event)]
struct PosClicked(BoardPosition);

fn on_board_clicked(
    clicked_pos: Trigger<PosClicked>,
    mut commands: Commands,
    selected_pos: Option<ResMut<SelectedPos>>,
    pegs: Query<(Entity, &BoardPosition), With<Peg>>,
) {
    let clicked = pegs.iter().find(|(_, p)| **p == clicked_pos.0);
    let selected =
        selected_pos.map(|selected_pos| pegs.iter().find(|(_, p)| **p == selected_pos.0).unwrap());

    match (selected, clicked) {
        (None, Some((clicked, clicked_pos))) => {
            commands.insert_resource(SelectedPos(*clicked_pos));
            commands.entity(clicked).insert(Selected);
        }
        (Some((selected, selected_pos)), clicked) => {
            commands.entity(selected).remove::<Selected>();
            commands.remove_resource::<SelectedPos>();
            commands.trigger(PegMoved { peg: selected }); // snap back
            if let Some((clicked, clicked_pos)) = clicked {
                if selected != clicked {
                    commands.insert_resource(SelectedPos(*clicked_pos));
                    commands.entity(clicked).insert(Selected);
                }
            } else {
                commands.trigger(RequestPegMove {
                    src: *selected_pos,
                    dst: clicked_pos.0,
                });
            }
        }
        _ => {}
    }
}

fn peg_selection_cursor(
    mut commands: Commands,
    window: Single<&Window, With<PrimaryWindow>>,
    camera_query: Single<(&Camera, &GlobalTransform)>,
) {
    if let Some(cursor_pos) = window.cursor_position() {
        let (camera, camera_transform) = *camera_query;
        let Some(world_pos_cursor) = viewport_to_world(cursor_pos, camera, camera_transform) else {
            return;
        };
        let board_pos = BoardPosition::from_world_space(world_pos_cursor.xy());
        commands.trigger(PosClicked(board_pos));
    }
}

#[derive(Resource, PartialEq, Eq)]
struct CurrentTouchId(u64);

fn peg_selection_touch(
    mut commands: Commands,
    touches: Res<Touches>,
    camera_query: Single<(&Camera, &GlobalTransform)>,
    current_touch_id: Option<Res<CurrentTouchId>>,
) {
    for touch in touches.iter() {
        if touches.just_pressed(touch.id())
            || Some(touch.id()) != current_touch_id.as_ref().map(|id| id.0)
        {
            let (camera, camera_transform) = *camera_query;
            let Some(world_pos) = viewport_to_world(touch.position(), camera, camera_transform)
            else {
                return;
            };
            let board_pos = BoardPosition::from_world_space(world_pos.xy());
            commands.trigger(PosClicked(board_pos));
        }
        commands.insert_resource(CurrentTouchId(touch.id()));
    }
}
