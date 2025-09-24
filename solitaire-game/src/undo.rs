use bevy::{
    input::common_conditions::input_just_pressed,
    prelude::*,
    window::{PrimaryWindow, RequestRedraw},
    winit::{EventLoopProxyWrapper, WakeUp},
};
use bevy_vector_shapes::prelude::*;

use crate::{CurrentSolution, viewport_to_world};

pub struct Buttons;

impl Plugin for Buttons {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, add_buttons);
        app.add_systems(
            Update,
            handle_button::<Undo, UndoEvent>.run_if(input_just_pressed(MouseButton::Left)),
        );
        app.add_systems(
            Update,
            handle_button::<Reset, ResetEvent>.run_if(input_just_pressed(MouseButton::Left)),
        );
        app.add_systems(Update, draw_buttons);
        app.add_systems(Update, update_button_pos);
        app.add_observer(do_undo);
        app.add_observer(do_reset);
        app.add_systems(Update, reset);
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
        ViewPortRelativeTranslation(Vec3::new(1.5, -0.5, 0.0)),
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
        ViewPortRelativeTranslation(Vec3::new(2.5, -0.5, 0.0)),
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
}

fn handle_button<T: Component, U: Default + Event>(
    window: Single<&Window, With<PrimaryWindow>>,
    camera: Single<(&Camera, &GlobalTransform)>,
    undo_button: Query<(&CircleButton, &Transform), With<T>>,
    mut commands: Commands,
) where
    T: Send + Sync,
{
    if let Some(cursor_pos) = window.cursor_position() {
        let (camera, transform) = *camera;
        let Some(world_pos) = viewport_to_world(cursor_pos, camera, transform) else {
            return;
        };
        for (button, transform) in undo_button {
            if world_pos.xy().distance(transform.translation.xy()) < button.radius {
                commands.trigger(U::default());
            }
        }
    }
}

fn do_undo(_: Trigger<UndoEvent>, mut solution: ResMut<CurrentSolution>) {
    info!("undo triggered!");
    if solution.0.len() > 0 {
        solution.0.pop();
    }
}

#[derive(Component)]
struct ResetComponent {
    elapsed: u64,
}

fn do_reset(
    _: Trigger<ResetEvent>,
    mut commands: Commands,
    reset_component: Query<&ResetComponent>,
) {
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
    mut request_redraw: EventWriter<RequestRedraw>,
) {
    let entity = *reset_entity;
    let mut reset = reset.get_mut(entity).unwrap();
    let ticks = reset.elapsed;
    reset.elapsed += 1;
    if ticks % 10 == 0 {
        if solution.0.len() > 0 {
            solution.0.pop();
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
