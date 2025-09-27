use bevy::{
    prelude::*,
    window::{PrimaryWindow, RequestRedraw},
};

use crate::{
    PegMoved, Selected,
    board::{BoardPosition, PEG_POS, PEG_POS_RAISED},
    viewport_to_world,
};

/// animates pegs to move to their current position smoothly
/// or follow the cursor
pub struct PegAnimation;

impl Plugin for PegAnimation {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, snap_to_board_grid);
        app.add_systems(PreUpdate, follow_mouse);
        app.add_observer(on_peg_move);
    }
}

#[derive(Component)]
struct SnapToBoardPosition;

fn on_peg_move(
    moved: Trigger<PegMoved>,
    mut commands: Commands,
    mut request_redraw: EventWriter<RequestRedraw>,
) {
    commands.entity(moved.peg).insert(SnapToBoardPosition);
    request_redraw.write(RequestRedraw);
}

fn snap_to_board_grid(
    mut commands: Commands,
    pegs: Query<Entity, With<SnapToBoardPosition>>,
    mut pos: Query<(&BoardPosition, &mut Transform), With<SnapToBoardPosition>>,
    mut request_redraw: EventWriter<RequestRedraw>,
) {
    for peg in pegs {
        if let Ok((board_pos, mut transform)) = pos.get_mut(peg) {
            let current = transform.translation;
            let target = Vec3::from(((*board_pos).to_world_space(), PEG_POS));
            let mut new_pos = current.lerp(target, 0.2);
            if new_pos.distance_squared(target) < 0.0001 {
                new_pos = target;
                commands.entity(peg).remove::<SnapToBoardPosition>();
            }
            transform.translation = new_pos;
            request_redraw.write(RequestRedraw);
        }
    }
}

fn follow_mouse(
    window: Single<&Window, With<PrimaryWindow>>,
    camera_query: Single<(&Camera, &GlobalTransform)>,
    transforms: Query<&mut Transform, With<Selected>>,
) {
    let (camera, camera_transform) = *camera_query;
    if let Some(cursor_pos) = window.cursor_position() {
        for mut transform in transforms {
            let current_z = transform.translation.z;
            let destination_z = PEG_POS_RAISED;
            if let Some(mut destination) = viewport_to_world(cursor_pos, camera, camera_transform) {
                destination.z = current_z.lerp(destination_z, 0.2);
                transform.translation = destination;
                // no need to RequestRedraw, since mouse movement already triggers a wakeup
            }
        }
    }
}
