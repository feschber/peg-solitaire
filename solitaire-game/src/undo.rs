use bevy::{
    ecs::entity_disabling::Disabled,
    input::common_conditions::input_just_pressed,
    prelude::*,
    window::{PrimaryWindow, RequestRedraw},
};
use bevy_vector_shapes::prelude::*;

use crate::{
    CurrentBoard, CurrentSolution, PegMoved, board::BoardPosition, hints::ToggleHints,
    viewport_to_world,
};

pub struct Buttons;

impl Plugin for Buttons {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, add_buttons);
        app.add_systems(
            Update,
            (
                handle_button::<Undo, UndoEvent>.run_if(input_just_pressed(MouseButton::Left)),
                handle_button::<Reset, ResetEvent>.run_if(input_just_pressed(MouseButton::Left)),
                handle_button::<Hints, ToggleHints>.run_if(input_just_pressed(MouseButton::Left)),
                handle_touch::<Undo, UndoEvent>,
                handle_touch::<Reset, ResetEvent>,
                handle_touch::<Hints, ToggleHints>,
            ),
        );
        app.add_systems(Update, (draw_buttons, update_button_pos, reset));
        app.add_observer(do_undo);
        app.add_observer(do_reset);
    }
}

#[derive(Component)]
struct ViewPortRelativeTranslation(Vec3);

#[derive(Event, Default)]
struct UndoEvent;

#[derive(Event, Default)]
struct ResetEvent;

#[derive(Component)]
struct CircleButton {
    color: Color,
    radius: f32,
}

#[derive(Component)]
struct Undo;

#[derive(Component)]
struct Reset;

#[derive(Component)]
struct Hints;

fn viewport_topleft_world_space(camera: &Camera, transform: &GlobalTransform) -> Option<Vec3> {
    camera.logical_viewport_rect().and_then(|view_port| {
        let top_left = view_port.min;
        let top_left_world_space = viewport_to_world(top_left, camera, transform);
        top_left_world_space
    })
}

fn update_button_pos(
    buttons: Query<(&ViewPortRelativeTranslation, &mut Transform), With<CircleButton>>,
    camera: Single<(&Camera, &GlobalTransform)>,
) {
    let (camera, transform) = *camera;
    let Some(viewport_topleft) = viewport_topleft_world_space(camera, transform) else {
        return;
    };
    for (rt, mut transform) in buttons {
        transform.translation = viewport_topleft + rt.0;
    }
}

fn add_buttons(mut commands: Commands, asset_server: Res<AssetServer>) {
    let font_awesome = asset_server.load("fonts/Font Awesome 7 Free-Solid-900.otf");
    let font_awesome = TextFont {
        font: font_awesome.clone(),
        font_size: 100.0,
        ..default()
    };
    // reset button
    commands.spawn((
        ViewPortRelativeTranslation(Vec3::new(1.5, -1.0, 0.0)),
        Transform::from_scale(Vec3::new(0.003, 0.003, 0.003)),
        CircleButton {
            color: Color::WHITE,
            radius: 0.4,
        },
        Text2d::new("\u{f2ea}".to_string()),
        TextColor(Color::BLACK.into()),
        font_awesome.clone(),
        Reset,
    ));
    // undo button
    commands.spawn((
        ViewPortRelativeTranslation(Vec3::new(2.5, -1.0, 0.0)),
        Transform::from_scale(Vec3::new(0.003, 0.003, 0.003)),
        CircleButton {
            color: Color::WHITE,
            radius: 0.4,
        },
        Text2d::new("\u{f060}".to_string()),
        TextColor(Color::BLACK.into()),
        font_awesome.clone(),
        Undo,
    ));
    // hints button
    commands.spawn((
        ViewPortRelativeTranslation(Vec3::new(3.5, -1.0, 0.0)),
        Transform::from_scale(Vec3::new(0.003, 0.003, 0.003)),
        CircleButton {
            color: Color::WHITE,
            radius: 0.4,
        },
        Text2d::new("\u{f0eb}".to_string()),
        TextColor(Color::BLACK.into()),
        font_awesome.clone(),
        Hints,
    ));
}

fn handle_button<'a, T: Component, U: Default + Event>(
    window: Single<&Window, With<PrimaryWindow>>,
    camera: Single<(&Camera, &GlobalTransform)>,
    button: Query<(&CircleButton, &Transform), With<T>>,
    mut commands: Commands,
) where
    T: Send + Sync,
    <U as bevy::prelude::Event>::Trigger<'a>: std::default::Default,
{
    if let Some(cursor_pos) = window.cursor_position() {
        let (camera, transform) = *camera;
        let Some(world_pos) = viewport_to_world(cursor_pos, camera, transform) else {
            return;
        };
        for (button, transform) in button {
            if world_pos.xy().distance(transform.translation.xy()) < button.radius {
                commands.trigger(U::default());
            }
        }
    }
}

fn handle_touch<'a, T: Component, U: Default + Event>(
    camera: Single<(&Camera, &GlobalTransform)>,
    button: Query<(&CircleButton, &Transform), With<T>>,
    mut commands: Commands,
    touches: Res<Touches>,
) where
    T: Send + Sync,
    <U as bevy::prelude::Event>::Trigger<'a>: std::default::Default,
{
    for pos in touches.iter_just_pressed().map(|t| t.position()) {
        let (camera, transform) = *camera;
        let Some(world_pos) = viewport_to_world(pos, camera, transform) else {
            return;
        };
        for (button, transform) in button {
            if world_pos.xy().distance(transform.translation.xy()) < button.radius {
                commands.trigger(U::default());
            }
        }
    }
}

fn do_undo(
    _: On<UndoEvent>,
    mut solution: ResMut<CurrentSolution>,
    mut board: ResMut<CurrentBoard>,
    mut commands: Commands,
) {
    info!("undo triggered!");
    if solution.0.len() > 0 {
        reverse_last_move(&mut solution, &mut board, &mut commands);
    }
}

fn reverse_last_move(
    solution: &mut CurrentSolution,
    board: &mut CurrentBoard,
    commands: &mut Commands,
) {
    let mov = solution.0.pop();
    let pegs = solution.1.pop().unwrap();
    board.0 = board.0.reverse_mov(mov);
    let prev_pos = BoardPosition::from(mov.pos);
    let skip_pos = BoardPosition::from(mov.skip);
    commands
        .entity(pegs.skipped)
        .remove::<Disabled>()
        .insert(skip_pos);
    commands.entity(pegs.moved).insert(prev_pos);
    commands.trigger(PegMoved { peg: pegs.moved });
    commands.trigger(PegMoved { peg: pegs.skipped });
}

#[derive(Component)]
struct ResetComponent {
    elapsed: u64,
}

fn do_reset(_: On<ResetEvent>, mut commands: Commands, reset_component: Query<&ResetComponent>) {
    info!("reset triggered!");
    if reset_component.is_empty() {
        commands.spawn(ResetComponent { elapsed: 0 });
    }
}

fn reset(
    reset_entity: Single<Entity, With<ResetComponent>>,
    mut reset: Query<&mut ResetComponent>,
    mut solution: ResMut<CurrentSolution>,
    mut commands: Commands,
    mut request_redraw: MessageWriter<RequestRedraw>,
    mut board: ResMut<CurrentBoard>,
) {
    let entity = *reset_entity;
    let mut reset = reset.get_mut(entity).unwrap();
    let ticks = reset.elapsed;
    reset.elapsed += 1;
    if ticks % 3 == 0 {
        if solution.0.len() > 0 {
            reverse_last_move(&mut solution, &mut board, &mut commands);
        } else {
            commands.entity(entity).despawn();
        }
    }
    request_redraw.write(RequestRedraw);
}

fn draw_buttons(mut painter: ShapePainter, buttons: Query<(&CircleButton, &Transform)>) {
    for (button, transform) in buttons {
        painter.set_translation(transform.translation - 0.1 * Vec3::Z);
        painter.set_color(button.color);
        painter.circle(button.radius);
    }
}
