use bevy::{camera::ScalingMode, prelude::*};
use bevy_vector_shapes::{prelude::ShapePainter, shapes::DiscPainter};
use solitaire_solver::Board;

use crate::{
    animation::PegAnimation,
    board::{BoardPlugin, BoardPosition, PEG_RADIUS},
    buttons::Buttons,
    fps_overlay::FpsOverlay,
    hints::HintsPlugin,
    input::Input,
    solver::Solver,
    stats::StatsPlugin,
    status::StatusPlugin,
    total_progress::TotalProgressPlugin,
    window::MainWindow,
};

mod animation;
mod board;
mod buttons;
mod fps_overlay;
mod hints;
mod input;
mod solver;
mod stats;
mod status;
mod total_progress;
mod window;

#[bevy_main]
fn main() {
    run()
}

pub fn run() {
    let mut app = App::new();
    app.add_plugins(MainWindow);
    app.add_plugins(PegSolitaire);
    #[cfg(not(target_arch = "wasm32"))]
    app.add_plugins(FpsOverlay);
    app.add_plugins(TotalProgressPlugin);
    app.run();
}

#[derive(Default, Resource)]
/// represents the currently active board
struct CurrentBoard(Board);

#[derive(Component)]
struct Selected;

fn camera_setup(mut commands: Commands) {
    commands.spawn(Camera2d);
}

fn scale_viewport(mut camera_query: Query<&mut Projection, With<Camera>>) {
    let Ok(mut projection) = camera_query.single_mut() else {
        return;
    };
    if let Projection::Orthographic(projection2d) = &mut *projection {
        projection2d.scaling_mode = ScalingMode::AutoMin {
            min_width: 8.,
            min_height: 8.,
        }
    }
}

fn update_solution(
    move_event: On<MoveEvent>,
    mut solution: ResMut<CurrentSolution>,
    board: Res<CurrentBoard>,
    mut commands: Commands,
) {
    solution.0.push(move_event.mov);
    solution.1.push(*move_event);
    if board.0.is_solved() {
        assert!(solution.0.len() == Board::SLOTS - 1);
        commands.trigger(SolutionEvent(solution.0.clone()));
    }
}

#[derive(Event)]
pub struct SolutionEvent(solitaire_solver::Solution);

#[derive(Default, Resource)]
struct CurrentSolution(solitaire_solver::Solution, Vec<MoveEvent>);

#[derive(Clone, Copy, Debug, Event)]
struct MoveEvent {
    mov: solitaire_solver::Move,
    moved: Entity,
    skipped: Entity,
}

#[allow(unused)]
#[derive(Event)]
struct PegMoved {
    peg: Entity,
}
struct PegSolitaire;

impl Plugin for PegSolitaire {
    fn build(&self, app: &mut App) {
        app.init_resource::<CurrentBoard>();
        app.init_resource::<CurrentSolution>();

        app.add_plugins(BoardPlugin);
        app.add_plugins(Solver);
        app.add_plugins(HintsPlugin);
        app.add_plugins(StatsPlugin);
        app.add_plugins(StatusPlugin);
        app.add_plugins(PegAnimation);
        app.add_plugins(Input);
        app.add_plugins(Buttons);

        app.add_observer(update_solution);
        app.add_systems(Startup, (camera_setup, scale_viewport).chain());
        app.add_systems(PostUpdate, highlight_selected);
        app.add_systems(PreUpdate, calc_view_port);
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

#[derive(Resource)]
pub struct WorldSpaceViewPort {
    pub top_left: Vec3,
    pub bottom_left: Vec3,
    pub top_right: Vec3,
    pub bottom_right: Vec3,
}

fn calc_view_port(mut commands: Commands, camera: Single<(&Camera, &GlobalTransform)>) {
    let (camera, transform) = *camera;
    if let Some(view_port) = camera.logical_viewport_rect() {
        let top_left = view_port.min;
        let bottom_right = view_port.max;
        let top_right = top_left + Vec2::new(view_port.width(), 0.);
        let bottom_left = top_left + Vec2::new(0., view_port.height());
        let Some(top_left) = viewport_to_world(top_left, camera, transform) else {
            return;
        };
        let Some(top_right) = viewport_to_world(top_right, camera, transform) else {
            return;
        };
        let Some(bottom_left) = viewport_to_world(bottom_left, camera, transform) else {
            return;
        };
        let Some(bottom_right) = viewport_to_world(bottom_right, camera, transform) else {
            return;
        };
        commands.insert_resource(WorldSpaceViewPort {
            top_left,
            bottom_left,
            top_right,
            bottom_right,
        });
    }
}
