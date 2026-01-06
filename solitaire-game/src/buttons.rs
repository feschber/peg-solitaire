use bevy::{
    ecs::entity_disabling::Disabled,
    input::common_conditions::{input_just_pressed, input_just_released},
    prelude::*,
    window::{PrimaryWindow, RequestRedraw},
};
use bevy_vector_shapes::prelude::*;

use crate::{
    CurrentBoard, CurrentSolution, PegMoved, WorldSpaceViewPort, board::BoardPosition,
    hints::ToggleHints, stats::ToggleStats, viewport_to_world,
};

pub struct Buttons;

impl Plugin for Buttons {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, add_buttons);
        app.add_systems(
            Update,
            (
                handle_button_press::<Undo, UndoEvent>
                    .run_if(input_just_pressed(MouseButton::Left)),
                handle_button_press::<Reset, ResetEvent>
                    .run_if(input_just_pressed(MouseButton::Left)),
                handle_button_release::<Undo>.run_if(input_just_released(MouseButton::Left)),
                handle_button_release::<Reset>.run_if(input_just_released(MouseButton::Left)),
                handle_toggle_press::<Hints, ToggleHints>
                    .run_if(input_just_pressed(MouseButton::Left)),
                handle_toggle_press::<Stats, ToggleStats>
                    .run_if(input_just_pressed(MouseButton::Left)),
                handle_touch_press::<Undo, UndoEvent>,
                handle_touch_press::<Reset, ResetEvent>,
                handle_touch_release::<Undo>,
                handle_touch_release::<Reset>,
                handle_touch_toggle::<Hints, ToggleHints>,
                handle_touch_toggle::<Stats, ToggleStats>,
            ),
        );
        app.add_systems(Update, (draw_buttons, update_button_pos));
        app.add_systems(Update, (draw_toggles, update_button_pos));
        app.add_systems(FixedUpdate, reset);
        app.add_observer(do_undo);
        app.add_observer(do_reset);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(unused)]
enum Pos {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

#[derive(Component)]
struct ViewPortRelativeTranslation(Pos, Vec3);

#[derive(Event, Default)]
struct UndoEvent;

#[derive(Event, Default)]
struct ResetEvent;

#[derive(Component)]
struct CircleButton {
    fg_color: Color,
    bg_color: Color,
    radius: f32,
}

#[derive(Component)]
struct ButtonState {
    clicked: bool,
    touched: Option<u64>,
}

#[derive(Component)]
struct ToggleState(bool);

#[derive(Component)]
struct Undo;

#[derive(Component)]
struct Reset;

#[derive(Component)]
struct Hints;

#[derive(Component)]
struct Stats;

fn update_button_pos(
    buttons: Query<(&ViewPortRelativeTranslation, &mut Transform), With<CircleButton>>,
    world_space_view_port: Option<Res<WorldSpaceViewPort>>,
) {
    if let Some(vp) = world_space_view_port {
        for (rt, mut transform) in buttons {
            let (pos, rt) = (rt.0, rt.1);
            match pos {
                Pos::TopLeft => transform.translation = vp.top_left + rt,
                Pos::TopRight => transform.translation = vp.top_right + rt,
                Pos::BottomLeft => transform.translation = vp.bottom_left + rt,
                Pos::BottomRight => transform.translation = vp.bottom_right + rt,
            }
        }
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
        ViewPortRelativeTranslation(Pos::TopLeft, Vec3::new(1.2, -1.0, 0.0)),
        Transform::from_scale(Vec3::new(0.003, 0.003, 0.003)),
        CircleButton {
            fg_color: Color::WHITE,
            bg_color: Color::BLACK,
            radius: 0.4,
        },
        ButtonState {
            clicked: false,
            touched: None,
        },
        Text2d::new("\u{f2ea}".to_string()),
        TextColor(Color::BLACK),
        font_awesome.clone(),
        Reset,
    ));
    // undo button
    commands.spawn((
        ViewPortRelativeTranslation(Pos::TopLeft, Vec3::new(1.2, -2.0, 0.0)),
        Transform::from_scale(Vec3::new(0.003, 0.003, 0.003)),
        CircleButton {
            fg_color: Color::WHITE,
            bg_color: Color::BLACK,
            radius: 0.3,
        },
        ButtonState {
            clicked: false,
            touched: None,
        },
        Text2d::new("\u{f060}".to_string()),
        TextColor(Color::BLACK),
        font_awesome.clone(),
        Undo,
    ));
    // hints button
    commands.spawn((
        ViewPortRelativeTranslation(Pos::TopRight, Vec3::new(-1., -1.0, 0.0)),
        Transform::from_scale(Vec3::new(0.003, 0.003, 0.003)),
        CircleButton {
            fg_color: Color::WHITE,
            bg_color: Color::BLACK,
            radius: 0.4,
        },
        ToggleState(false),
        Text2d::new("\u{f0eb}".to_string()),
        TextColor(Color::BLACK),
        font_awesome.clone(),
        Hints,
    ));
    commands.spawn((
        ViewPortRelativeTranslation(Pos::TopRight, Vec3::new(-2., -1.0, 1.0)),
        Transform::from_scale(Vec3::new(0.003, 0.003, 0.003)),
        CircleButton {
            fg_color: Color::WHITE,
            bg_color: Color::BLACK,
            radius: 0.4,
        },
        ToggleState(true),
        Text2d::new("\u{f5dc}".to_string()),
        TextColor(Color::WHITE),
        font_awesome.clone(),
        Stats,
    ));
}

fn handle_button_press<'a, T, U: Default + Event>(
    window: Single<&Window, With<PrimaryWindow>>,
    camera: Single<(&Camera, &GlobalTransform)>,
    mut button: Query<(&CircleButton, &mut ButtonState, &Transform), With<T>>,
    mut commands: Commands,
) where
    T: Component + Send + Sync,
    <U as bevy::prelude::Event>::Trigger<'a>: std::default::Default,
{
    if let Some(cursor_pos) = window.cursor_position() {
        let (camera, transform) = *camera;
        let Some(world_pos) = viewport_to_world(cursor_pos, camera, transform) else {
            return;
        };
        for (button, mut state, transform) in &mut button {
            if world_pos.xy().distance(transform.translation.xy()) < button.radius {
                commands.trigger(U::default());
                state.clicked = true;
            }
        }
    }
}

fn handle_button_release<T>(mut button: Query<&mut ButtonState, With<T>>)
where
    T: Component + Send + Sync,
{
    for mut state in &mut button {
        state.clicked = false;
    }
}

fn handle_toggle_press<'a, T, U: Default + Event>(
    window: Single<&Window, With<PrimaryWindow>>,
    camera: Single<(&Camera, &GlobalTransform)>,
    mut button: Query<(&CircleButton, &mut ToggleState, &Transform), With<T>>,
    mut commands: Commands,
) where
    T: Component + Send + Sync,
    <U as bevy::prelude::Event>::Trigger<'a>: std::default::Default,
{
    if let Some(cursor_pos) = window.cursor_position() {
        let (camera, transform) = *camera;
        let Some(world_pos) = viewport_to_world(cursor_pos, camera, transform) else {
            return;
        };
        for (button, mut state, transform) in &mut button {
            if world_pos.xy().distance(transform.translation.xy()) < button.radius {
                state.0 = !state.0;
                commands.trigger(U::default());
            }
        }
    }
}

fn handle_touch_press<'a, T, U: Default + Event>(
    camera: Single<(&Camera, &GlobalTransform)>,
    mut buttons: Query<(&CircleButton, &mut ButtonState, &Transform), With<T>>,
    mut commands: Commands,
    touches: Res<Touches>,
) where
    T: Component + Send + Sync,
    <U as bevy::prelude::Event>::Trigger<'a>: std::default::Default,
{
    for touch in touches.iter_just_pressed() {
        let (camera, transform) = *camera;
        let Some(world_pos) = viewport_to_world(touch.position(), camera, transform) else {
            return;
        };
        for (button, mut state, transform) in &mut buttons {
            if world_pos.xy().distance(transform.translation.xy()) < button.radius {
                commands.trigger(U::default());
                state.touched = Some(touch.id());
            }
        }
    }
}

fn handle_touch_release<'a, T>(mut buttons: Query<&mut ButtonState, With<T>>, touches: Res<Touches>)
where
    T: Component + Send + Sync,
{
    for released_id in touches.iter_just_released().map(|t| t.id()) {
        for mut state in &mut buttons {
            if let Some(id) = state.touched {
                if id == released_id {
                    state.touched = None;
                }
            }
        }
    }
}

fn handle_touch_toggle<'a, T, U: Default + Event>(
    camera: Single<(&Camera, &GlobalTransform)>,
    mut button: Query<(&CircleButton, &mut ToggleState, &Transform), With<T>>,
    mut commands: Commands,
    touches: Res<Touches>,
) where
    T: Component + Send + Sync,
    <U as bevy::prelude::Event>::Trigger<'a>: std::default::Default,
{
    for pos in touches.iter_just_pressed().map(|t| t.position()) {
        let (camera, transform) = *camera;
        let Some(world_pos) = viewport_to_world(pos, camera, transform) else {
            return;
        };
        for (button, mut state, transform) in &mut button {
            if world_pos.xy().distance(transform.translation.xy()) < button.radius {
                commands.trigger(U::default());
                state.0 = !state.0;
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
    if !solution.0.is_empty() {
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
    if ticks.is_multiple_of(2) {
        if !solution.0.is_empty() {
            reverse_last_move(&mut solution, &mut board, &mut commands);
        } else {
            commands.entity(entity).despawn();
        }
    }
    request_redraw.write(RequestRedraw);
}

fn draw_buttons(
    mut painter: ShapePainter,
    mut buttons: Query<(&CircleButton, &ButtonState, &Transform, &mut TextColor)>,
) {
    for (button, state, transform, mut col) in &mut buttons {
        painter.set_translation(transform.translation - 0.1 * Vec3::Z);
        if state.clicked || state.touched.is_some() {
            *col = TextColor(button.bg_color);
            painter.set_color(button.fg_color);
        } else {
            *col = TextColor(button.fg_color);
            painter.set_color(button.bg_color);
        }
        painter.circle(button.radius);
    }
}

fn draw_toggles(
    mut painter: ShapePainter,
    mut buttons: Query<(&CircleButton, &ToggleState, &Transform, &mut TextColor)>,
) {
    for (button, state, transform, mut col) in &mut buttons {
        painter.set_translation(transform.translation - 0.1 * Vec3::Z);
        if state.0 {
            *col = TextColor(button.bg_color);
            painter.set_color(button.fg_color);
        } else {
            *col = TextColor(button.fg_color);
            painter.set_color(button.bg_color);
        }
        painter.circle(button.radius);
    }
}
