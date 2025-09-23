use bevy::{prelude::*, winit::WinitSettings};
use bevy_vector_shapes::{prelude::ShapePainter, shapes::DiscPainter};
use solitaire_solver::Board;

use crate::{
    board::{BoardPlugin, BoardPosition, PEG_RADIUS},
    fps_overlay::FpsOverlay,
    hints::HintsPlugin,
    input::Input,
    movement::Movement,
    solver::Solver,
    stats::StatsPlugin,
    status::StatusPlugin,
    undo::Buttons,
    window::MainWindow,
};

mod board;
mod fps_overlay;
mod hints;
mod input;
mod movement;
mod solver;
mod stats;
mod status;
mod undo;
mod window;

#[bevy_main]
fn main() {
    run()
}

pub fn run() {
    App::new()
        .add_plugins(MainWindow)
        .add_plugins(FpsOverlay)
        .add_plugins(PegSolitaire)
        .run();
}

#[derive(Default, Resource)]
/// represents the currently active board
struct CurrentBoard(Board);

#[derive(Component)]
struct Selected;

#[derive(Component)]
struct SnapToBoardPosition;

fn camera_setup(mut commands: Commands) {
    commands.spawn(Camera2d);
}

fn scale_viewport(mut camera_query: Query<&mut Projection, With<Camera>>) {
    let Ok(mut projection) = camera_query.single_mut() else {
        return;
    };
    if let Projection::Orthographic(projection2d) = &mut *projection {
        projection2d.scaling_mode = bevy::render::camera::ScalingMode::AutoMin {
            min_width: 8.,
            min_height: 8.,
        }
    }
}

fn update_solution(move_event: Trigger<PegMoved>, mut solution: ResMut<CurrentSolution>) {
    solution.0.push(move_event.mov);
}

#[derive(Default, Resource)]
struct CurrentSolution(solitaire_solver::Solution);

#[allow(unused)]
#[derive(Event)]
struct PegMoved {
    prev_pos: BoardPosition,
    new_pos: BoardPosition,
    mov: solitaire_solver::Move,
}
struct PegSolitaire;

impl Plugin for PegSolitaire {
    fn build(&self, app: &mut App) {
        app.insert_resource(WinitSettings::desktop_app());

        app.init_resource::<CurrentBoard>();
        app.init_resource::<CurrentSolution>();

        app.add_plugins(BoardPlugin);
        app.add_plugins(Solver);
        app.add_plugins(HintsPlugin);
        app.add_plugins(StatsPlugin);
        app.add_plugins(StatusPlugin);
        app.add_plugins(Movement);
        app.add_plugins(Input);
        app.add_plugins(Buttons);

        app.add_observer(update_solution);
        // app.add_systems(Startup, camera_setup_3d);
        app.add_systems(Startup, (camera_setup, scale_viewport).chain());
        app.add_systems(Update, highlight_selected);
    }
}

fn highlight_selected(mut painter: ShapePainter, selected: Query<&Transform, With<Selected>>) {
    for selected in selected {
        painter.set_translation(selected.translation - Vec3::Z * 0.1);
        painter.set_color(Color::WHITE);
        painter.circle(PEG_RADIUS * 1.1);
    }
}

fn viewport_to_world(
    pos: Vec2,
    camera: &Camera,
    camera_transform: &GlobalTransform,
) -> Option<Vec3> {
    let ray = camera.viewport_to_world(camera_transform, pos).ok()?;
    let ground_plane = InfinitePlane3d::new(Vec3::Z);
    let distance = ray.intersect_plane(Vec3::ZERO, ground_plane)?;
    let point = ray.get_point(distance);
    Some(point)
}
