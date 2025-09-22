use bevy::{input::common_conditions::input_just_pressed, prelude::*, window::PrimaryWindow};
use solitaire_solver::Board;

use crate::{
    CurrentBoard, PegMoved, Selected, SnapToBoardPosition,
    board::{BoardPosition, Peg},
    hints::ToggleHints,
    viewport_to_world,
};

pub struct Input;

impl Plugin for Input {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            peg_selection_cursor.run_if(input_just_pressed(MouseButton::Left)),
        );
        app.add_systems(Startup, touch_hack);
        app.add_systems(Update, peg_selection_touch);
    }
}

fn handle_click(
    commands: &mut Commands,
    pegs: Query<Entity, With<Peg>>,
    selected: Query<&Selected>,
    positions: &mut Query<&mut BoardPosition>,
    board: &mut ResMut<CurrentBoard>,
    cursor_pos: Vec2,
    camera_query: &Single<(&Camera, &GlobalTransform)>,
) {
    let (camera, camera_transform) = **camera_query;
    let Some(world_pos_cursor) = viewport_to_world(cursor_pos, camera, camera_transform) else {
        return;
    };
    let nearest_peg = BoardPosition::from_world_space(world_pos_cursor.xy());
    if !Board::inbounds(nearest_peg.into()) {
        commands.trigger(ToggleHints);
    }
    for entity in pegs {
        if let Ok(mut board_pos) = positions.get_mut(entity) {
            let mut entity_commands = commands.entity(entity);
            if selected.contains(entity) {
                entity_commands.remove::<Selected>();
                entity_commands.insert(SnapToBoardPosition);

                // allow swapping pegs
                let current = (*board_pos).into();
                let destination = nearest_peg.into();
                if !Board::inbounds(destination) {
                    continue;
                }
                if board.0.occupied(destination) {
                    // *board_pos = nearest_peg;
                } else if let Some(mov) = board.0.is_legal_move(current, destination) {
                    println!("{mov}");
                    // update board
                    board.0 = board.0.mov(mov);

                    // update peg position
                    let prev_pos = *board_pos;
                    let new_pos = nearest_peg;
                    *board_pos = nearest_peg;
                    commands.trigger(PegMoved {
                        prev_pos,
                        new_pos,
                        mov,
                    });
                    // remove skipped peg
                    for peg in pegs {
                        if let Ok(b) = positions.get(peg) {
                            if b.y == mov.skip.0 && b.x == mov.skip.1 {
                                commands.entity(peg).despawn();
                            }
                        }
                    }
                } else {
                    println!("illegal move!");
                }
            } else {
                if *board_pos == nearest_peg {
                    entity_commands.insert(Selected);
                    entity_commands.remove::<SnapToBoardPosition>();
                }
            }
        }
    }
}

fn peg_selection_cursor(
    mut commands: Commands,
    window: Single<&Window, With<PrimaryWindow>>,
    pegs: Query<Entity, With<Peg>>,
    mut positions: Query<&mut BoardPosition>,
    selected: Query<&Selected>,
    mut board: ResMut<CurrentBoard>,
    camera_query: Single<(&Camera, &GlobalTransform)>,
) {
    if let Some(cursor_pos) = window.cursor_position() {
        handle_click(
            &mut commands,
            pegs,
            selected,
            &mut positions,
            &mut board,
            cursor_pos,
            &camera_query,
        )
    }
}

fn peg_selection_touch(
    mut commands: Commands,
    pegs: Query<Entity, With<Peg>>,
    mut positions: Query<&mut BoardPosition>,
    selected: Query<&Selected>,
    mut board: ResMut<CurrentBoard>,
    touches: Res<Touches>,
    mut current_touch_id: Query<&mut CurrentTouchId>,
    camera_query: Single<(&Camera, &GlobalTransform)>,
) {
    let mut current_touch_id = current_touch_id.single_mut().unwrap();
    for touch in touches.iter() {
        if touch.id() != current_touch_id.0 || touches.just_pressed(touch.id()) {
            current_touch_id.0 = touch.id();
            info!("touch position: {:?}", touch.position());
            handle_click(
                &mut commands,
                pegs,
                selected,
                &mut positions,
                &mut board,
                touch.position(),
                &camera_query,
            )
        }
    }
}

fn touch_hack(mut commands: Commands) {
    commands.spawn(CurrentTouchId(u64::MAX));
}

#[derive(Component)]
struct CurrentTouchId(u64);
