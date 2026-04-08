use bevy::{
    input::common_conditions::{input_just_pressed, input_just_released},
    prelude::*,
    window::{PrimaryWindow, RequestRedraw},
    winit::{EventLoopProxyWrapper, WinitUserEvent},
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
        app.add_systems(PreUpdate, keyboard_input);
        app.add_systems(PreUpdate, wake_on_touch_release);
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
    mut request_redraw: MessageWriter<RequestRedraw>,
) {
    let (camera, camera_transform) = *camera_query;
    if let Some(cursor_pos) = window.cursor_position() {
        if let Some(world_pos_cursor) = viewport_to_world(cursor_pos, camera, camera_transform) {
            let board_pos = BoardPosition::from_world_space(world_pos_cursor.xy());
            if let Some(peg) = pegs.iter().find(|(_, p)| **p == board_pos).map(|(p, _)| p) {
                commands.entity(peg).insert(Selected);
                request_redraw.write(RequestRedraw);
            }
        };
    }
}

fn release_peg(
    mut commands: Commands,
    window: Single<&Window, With<PrimaryWindow>>,
    camera_query: Single<(&Camera, &GlobalTransform)>,
    selected_pegs: Query<(Entity, &BoardPosition), (With<Peg>, With<Selected>)>,
    mut request_redraw: MessageWriter<RequestRedraw>,
) {
    let (camera, camera_transform) = *camera_query;
    if let Some(cursor_pos) = window.cursor_position() {
        if let Some(world_pos_cursor) = viewport_to_world(cursor_pos, camera, camera_transform) {
            let board_pos = BoardPosition::from_world_space(world_pos_cursor.xy());
            for (selected_peg, &current_pos) in selected_pegs {
                move_peg(&mut commands, selected_peg, current_pos, board_pos);
            }
            request_redraw.write(RequestRedraw);
        };
    }
}

fn peg_selection_touch(
    mut commands: Commands,
    touches: Res<Touches>,
    camera_query: Single<(&Camera, &GlobalTransform)>,
    pegs: Query<(Entity, &BoardPosition), With<Peg>>,
    selected_pegs: Query<(Entity, &BoardPosition), (With<Peg>, With<Selected>)>,
    mut request_redraw: MessageWriter<RequestRedraw>,
) {
    let (camera, camera_transform) = *camera_query;
    for touch in touches.iter_just_pressed() {
        if let Some(world_pos) = viewport_to_world(touch.position(), camera, camera_transform) {
            let board_pos = BoardPosition::from_world_space(world_pos.xy());
            if let Some(peg) = pegs.iter().find(|(_, p)| **p == board_pos).map(|(p, _)| p) {
                commands.entity(peg).insert(Selected);
            }
        }
        request_redraw.write(RequestRedraw);
    }
    for touch in touches
        .iter_just_released()
        .chain(touches.iter_just_canceled())
    {
        if let Some(world_pos) = viewport_to_world(touch.position(), camera, camera_transform) {
            let board_pos = BoardPosition::from_world_space(world_pos.xy());
            for (selected_peg, &current_pos) in selected_pegs {
                move_peg(&mut commands, selected_peg, current_pos, board_pos);
            }
        }
        request_redraw.write(RequestRedraw);
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

fn keyboard_input(
    mut commands: Commands,
    window: Single<&Window, With<PrimaryWindow>>,
    camera_query: Single<(&Camera, &GlobalTransform)>,
    pegs: Query<(Entity, &BoardPosition), With<Peg>>,
    keys: Res<ButtonInput<KeyCode>>,
) {
    let (camera, transform) = *camera_query;
    let Some(cursor_pos) = window.cursor_position() else {
        return;
    };
    let Some(world_pos_cursor) = viewport_to_world(cursor_pos, camera, transform) else {
        return;
    };
    let board_pos = BoardPosition::from_world_space(world_pos_cursor.xy());
    let Some((peg, pos)) = pegs
        .iter()
        .find(|(_, p)| **p == board_pos)
        .map(|(peg, pos)| (peg, *pos))
    else {
        return;
    };

    if keys.just_pressed(KeyCode::KeyW) || keys.just_pressed(KeyCode::ArrowUp) {
        move_peg(&mut commands, peg, pos, pos + BoardPosition { x: 0, y: -2 });
    }
    if keys.just_pressed(KeyCode::KeyS) || keys.just_pressed(KeyCode::ArrowDown) {
        move_peg(&mut commands, peg, pos, pos + BoardPosition { x: 0, y: 2 });
    }
    if keys.just_pressed(KeyCode::KeyA) || keys.just_pressed(KeyCode::ArrowLeft) {
        move_peg(&mut commands, peg, pos, pos + BoardPosition { x: -2, y: 0 });
    }
    if keys.just_pressed(KeyCode::KeyD) || keys.just_pressed(KeyCode::ArrowRight) {
        move_peg(&mut commands, peg, pos, pos + BoardPosition { x: 2, y: 0 });
    }
}

fn wake_on_touch_release(touches: Res<Touches>, wake: Res<EventLoopProxyWrapper>) {
    for _ in touches.iter_just_released() {
        wake.send_event(WinitUserEvent::WakeUp).unwrap();
    }
}
