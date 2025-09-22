use bevy::{
    input::common_conditions::input_just_pressed,
    prelude::*,
    window::{PrimaryWindow, RequestRedraw},
    winit::WinitSettings,
};
use bevy_vector_shapes::{prelude::ShapePainter, shapes::DiscPainter};
use solitaire_solver::Board;

use crate::{
    board::{BOARD_POS, BoardPlugin, BoardPosition, PEG_POS, PEG_POS_RAISED, PEG_RADIUS, Peg},
    fps_overlay::FpsOverlay,
    hints::{HintsPlugin, ToggleHints},
    solver::Solver,
    stats::StatsPlugin,
    status::StatusPlugin,
    window::MainWindow,
};

mod board;
mod fps_overlay;
mod hints;
mod solver;
mod stats;
mod status;
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
    mut request_redraw: EventWriter<RequestRedraw>,
) {
    let (camera, camera_transform) = *camera_query;
    if let Some(cursor_pos) = window.cursor_position() {
        for mut transform in transforms {
            let current_z = transform.translation.z;
            let destination_z = PEG_POS_RAISED;
            if let Some(mut destination) = viewport_to_world(cursor_pos, camera, camera_transform) {
                destination.z = current_z.lerp(destination_z, 0.2);
                transform.translation = destination;
                request_redraw.write(RequestRedraw);
            }
        }
    }
}

#[allow(unused)]
fn camera_setup_3d(mut commands: Commands /*  asset_server: &AssetServer */) {
    commands.spawn((
        Camera3d::default(),
        Camera {
            hdr: true,
            ..default()
        },
        camera_transform_3d(),
        DistanceFog {
            color: Color::srgb_u8(43, 44, 47),
            falloff: FogFalloff::Linear {
                start: 10.,
                end: 50.,
            },
            ..default()
        },
        // EnvironmentMapLight {
        //     diffuse_map: asset_server.load("environment_maps/pisa_diffuse_rgb9e5_zstd.ktx2"),
        //     specular_map: asset_server.load("environment_maps/pisa_specular_rgb9e5_zstd.ktx2"),
        //     intensity: 2000.0,
        //     ..default()
        // },
    ));
}

fn setup_3d_meshes(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.spawn((
        PointLight {
            intensity: 15_000_000.0,
            shadows_enabled: true,
            ..default()
        },
        Transform::from_xyz(-5.0, 9.0, 8.),
    ));
    // commands.spawn((
    //     DirectionalLight {
    //         illuminance: light_consts::lux::OVERCAST_DAY,
    //         shadows_enabled: true,
    //         ..default()
    //     },
    //     Transform {
    //         translation: Vec3::new(0.0, 0.0, 0.0),
    //         rotation: Quat::from_rotation_z(-2.5 * PI / 4.),
    //         ..default()
    //     },
    // ));
    let ground_plane = Plane3d::new(Vec3::Z, Vec2::splat(4.));
    commands.spawn((
        Mesh3d(meshes.add(ground_plane.mesh())),
        MeshMaterial3d(materials.add(Color::srgb(0.8, 0.8, 0.8))),
        Transform::from_xyz(0.0, 0.0, BOARD_POS),
    ));
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

        app.add_observer(update_solution);
        app.add_systems(Startup, setup_3d_meshes);
        // app.add_systems(Startup, camera_setup_3d);
        app.add_systems(Startup, (camera_setup, scale_viewport).chain());
        app.add_systems(Update, snap_to_board_grid);
        app.add_systems(Update, follow_mouse);
        app.add_systems(
            Update,
            peg_selection_cursor.run_if(input_just_pressed(MouseButton::Left)),
        );
        app.add_systems(Startup, touch_hack);
        app.add_systems(Update, peg_selection_touch);
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

fn camera_transform_3d() -> Transform {
    Transform::from_xyz(6., 3., 10.).looking_at(Vec3::new(0.0, 0.0, 0.0), Vec3::Z)
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

#[allow(unused)]
fn cursor_to_world_2d(
    pos: Vec2,
    camera: &Camera,
    camera_transform: &GlobalTransform,
) -> Option<Vec2> {
    Some(camera.viewport_to_world_2d(camera_transform, pos).ok()?)
}
