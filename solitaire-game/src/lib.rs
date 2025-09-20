use std::{
    collections::{HashMap, HashSet},
    f32, u64,
};

use bevy::{
    dev_tools::fps_overlay::{FpsOverlayConfig, FpsOverlayPlugin},
    ecs::world::CommandQueue,
    input::common_conditions::input_just_pressed,
    log::{Level, LogPlugin},
    render::mesh::{CircleMeshBuilder, SphereKind, SphereMeshBuilder},
    sprite::Anchor,
    tasks::{AsyncComputeTaskPool, Task},
    text::TextBounds,
    window::{PrimaryWindow, RequestRedraw, WindowMode, WindowTheme},
    winit::WinitSettings,
};
use bevy::{prelude::*, window::WindowThemeChanged};
use bevy_vector_shapes::{
    Shape2dPlugin,
    prelude::ShapePainter,
    shapes::{DiscPainter, LinePainter, ThicknessType},
};
use futures_lite::future::{self, block_on};
use solitaire_solver::{Board, Dir};

#[bevy_main]
fn main() {
    run()
}

fn update_window_theme(
    theme_changed: Trigger<WindowThemeChanged>,
    mut clear_color: ResMut<ClearColor>,
) {
    info!("Theme Changed!");
    match theme_changed.event().theme {
        WindowTheme::Light => *clear_color = ClearColor(Color::WHITE),
        WindowTheme::Dark => *clear_color = ClearColor(Color::BLACK),
    }
}

pub fn run() {
    App::new()
        .insert_resource(ClearColor(Color::BLACK))
        .add_plugins(
            DefaultPlugins
                .set(LogPlugin {
                    // This will show some log events from Bevy to the native logger.
                    level: Level::INFO,
                    filter: "wgpu=error,bevy_render=info,bevy_ecs=trace".to_string(),
                    ..Default::default()
                })
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        // title: "Peg Solitaire".into(),
                        fit_canvas_to_parent: true,
                        prevent_default_event_handling: false,
                        desired_maximum_frame_latency: core::num::NonZero::new(1u32),
                        present_mode: bevy::window::PresentMode::AutoVsync,
                        mode: WindowMode::BorderlessFullscreen(MonitorSelection::Primary),
                        // on iOS, gestures must be enabled.
                        // This doesn't work on Android
                        recognize_rotation_gesture: true,
                        // Only has an effect on iOS
                        prefers_home_indicator_hidden: true,
                        // Only has an effect on iOS
                        prefers_status_bar_hidden: true,
                        ..default()
                    }),
                    ..default()
                }),
        )
        .add_plugins(Shape2dPlugin::default())
        .add_plugins(PegSolitaire)
        .add_plugins(FpsOverlayPlugin {
            config: FpsOverlayConfig {
                text_config: TextFont {
                    font_size: 10.0,
                    ..default()
                },
                text_color: Color::WHITE,
                refresh_interval: core::time::Duration::from_millis(100),
                enabled: false,
            },
        })
        .run();
}

#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
struct BoardPosition {
    x: i64,
    y: i64,
}

impl From<BoardPosition> for Vec2 {
    fn from(board_position: BoardPosition) -> Self {
        Vec2::new(board_position.x as f32, board_position.y as f32)
    }
}

impl From<Vec2> for BoardPosition {
    fn from(v: Vec2) -> Self {
        BoardPosition {
            x: v.x.round() as _,
            y: v.y.round() as _,
        }
    }
}

#[derive(Component)]
struct BoardComponent {
    /// represents the currently active board
    board: Board,
}

#[derive(Resource)]
struct FeasibleConstellations(HashSet<Board>);

#[derive(Resource)]
struct RandomMoveChances(HashMap<Board, f64>);

#[derive(Component)]
struct BackgroundTask {
    task: Task<CommandQueue>,
}

impl Default for BoardComponent {
    fn default() -> Self {
        let board = Board::default();
        BoardComponent { board }
    }
}

fn create_solution_dag(mut commands: Commands) {
    info!("calculating feasible constellations ...");
    let thread_pool = AsyncComputeTaskPool::get();
    let entity = commands.spawn_empty().id();
    let task = thread_pool.spawn(async move {
        #[cfg(feature = "solution_cache")]
        let feasible = solution_cache::load_solutions();
        #[cfg(not(feature = "solution_cache"))]
        let feasible = solitaire_solver::calculate_all_solutions(None);

        let feasible_hashset = HashSet::from_iter(feasible.iter().copied());
        let mut command_queue = CommandQueue::default();
        command_queue.push(move |world: &mut World| {
            world.insert_resource(FeasibleConstellations(feasible_hashset));
            info!("inserting feasible constellations...");
            world.trigger(UpdateStats);
            world.entity_mut(entity).remove::<BackgroundTask>();
        });
        command_queue
    });
    commands.entity(entity).insert(BackgroundTask { task });
}

fn calculate_random_move_chances(mut commands: Commands, feasible: Res<FeasibleConstellations>) {
    info!("calculating P(\"success by random moves\") ...");
    let thread_pool = AsyncComputeTaskPool::get();
    let entity = commands.spawn_empty().id();
    let feasible = feasible.0.clone();
    let task = thread_pool.spawn(async move {
        let feasible = feasible.iter().copied().collect();
        let p_random_chance = solitaire_solver::calculate_p_random_chance_success(feasible);

        let mut command_queue = CommandQueue::default();
        command_queue.push(move |world: &mut World| {
            world.insert_resource(RandomMoveChances(p_random_chance));
            world.entity_mut(entity).remove::<BackgroundTask>();
        });
        command_queue
    });
    commands.entity(entity).insert(BackgroundTask { task });
}

fn poll_task(mut commands: Commands, mut solution_task: Query<&mut BackgroundTask>) {
    for mut task in &mut solution_task {
        if let Some(mut commands_queue) = block_on(future::poll_once(&mut task.task)) {
            commands.append(&mut commands_queue)
        }
    }
}

fn setup_board(mut commands: Commands) {
    commands.spawn(BoardComponent::default());
}

#[derive(Component)]
struct BoardMarker;

#[derive(Component)]
struct Peg;

const BOARD_POS: f32 = 0.0;
const MARKER_POS: f32 = 0.1;
const PEG_POS: f32 = 0.2;
const PEG_POS_RAISED: f32 = 1.0;

fn spawn_pegs(
    mut commands: Commands,
    board: Query<&BoardComponent>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut color_materials: ResMut<Assets<ColorMaterial>>,
) {
    // the board itself
    commands.spawn((
        BoardMarker,
        Name::new("board"),
        Transform::from_translation(Vec3::new(0., 0., BOARD_POS)),
        Mesh2d(meshes.add(CircleMeshBuilder::new(4., 1000).build())),
        MeshMaterial2d(color_materials.add(Color::WHITE.with_luminance(0.02))),
    ));

    let board = board.single().expect("board").board;
    let sphere = Mesh3d(
        meshes.add(
            SphereMeshBuilder::new(
                1. / (2. * GOLDEN_RATIO),
                SphereKind::Ico { subdivisions: 10 },
            )
            .build(),
        ),
    );
    let hole_circle = Mesh2d(meshes.add(CircleMeshBuilder::new(HOLE_RADIUS, 1000).build()));
    let peg_circle = Mesh2d(meshes.add(CircleMeshBuilder::new(PEG_RADIUS, 1000).build()));
    let hole_color = Color::WHITE.with_luminance(0.01);
    let hole_material = materials.add(hole_color);
    let hole_color_material = color_materials.add(hole_color);
    for y in 0..Board::SIZE {
        for x in 0..Board::SIZE {
            let board_pos = BoardPosition { y, x };
            let world_pos = board_to_world(board_pos);
            if Board::inbounds((y, x)) {
                // spawn holes
                commands.spawn((
                    hole_circle.clone(),
                    Transform::from_translation((world_pos, BOARD_POS).into()),
                    MeshMaterial3d::from(hole_material.clone()),
                    MeshMaterial2d::from(hole_color_material.clone()),
                ));
            }

            // spawn pegs
            let col = Color::hsl(((y * 7 + x) * 16) as f32, 1., 0.7);
            if board.occupied((y, x)) {
                commands.spawn((
                    sphere.clone(),
                    peg_circle.clone(),
                    MeshMaterial3d(materials.add(col)),
                    MeshMaterial2d(color_materials.add(col)),
                    BoardPosition { y, x },
                    Transform::from_translation((world_pos, PEG_POS).into()),
                    Peg,
                ));
            }
        }
    }
}

const GOLDEN_RATIO: f32 = 1.618033988749;
const PEG_RADIUS: f32 = 1. / (2. * GOLDEN_RATIO);
const HOLE_RADIUS: f32 = 0.9 * PEG_RADIUS;

#[derive(Component)]
struct FollowMouse;

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

#[derive(Component)]
struct NextMoveChanceText;

#[derive(Component)]
struct OverallSuccessRatio;

fn update_overall_success(
    _trigger: Trigger<UpdateStats>,
    overall_success_text: Query<Entity, With<OverallSuccessRatio>>,
    board: Query<&BoardComponent>,
    p_success: Option<Res<RandomMoveChances>>,
    mut writer: TextUiWriter,
    mut request_redraw: EventWriter<RequestRedraw>,
) {
    let Some(p_success) = p_success else {
        return;
    };
    let p_success = &p_success.0;
    let board = board.single().expect("board").board;
    let board = board.normalize();

    let p_success = *p_success.get(&board).unwrap_or(&0.0);
    // let p = num_rational::BigRational::from_float(p_success).unwrap();
    // let odds = p.clone() / (num_rational::BigRational::from_float(1.0).unwrap() - p.clone());
    for text in overall_success_text {
        if p_success > 0. {
            let inverse = 1. / p_success;
            let mut str = format!("1/{inverse:.0}");
            if str.len() > 4 {
                str = format!("\n{str}");
            }
            *writer.text(text, 1) = str;
        } else {
            *writer.text(text, 1) = format!("0");
        }
    }
    request_redraw.write(RequestRedraw);
}

fn update_stats_on_solution(mut commands: Commands) {
    commands.trigger(UpdateStats);
}

fn update_stats_on_move(_trigger: Trigger<PegMoved>, mut commands: Commands) {
    commands.trigger(UpdateStats);
}

fn update_solution(move_event: Trigger<PegMoved>, mut solution: ResMut<Solution>) {
    solution.0.push(move_event.mov);
}

fn draw_solution(
    solution: Res<Solution>,
    mut painter: ShapePainter,
    camera_query: Single<(&Camera, &GlobalTransform)>,
) {
    let cam = camera_query;
    if let Some(view_port) = cam.0.logical_viewport_rect() {
        let y_size = view_port.max.y - view_port.min.y;
        let x_size = view_port.max.x - view_port.min.x;

        let (start, end) = if y_size > x_size {
            let pos_vp = (Vec2::new(view_port.min.x, view_port.max.y), view_port.max);
            let pos_ws = (
                viewport_to_world(pos_vp.0, cam.0, cam.1).unwrap_or_default()
                    + Vec3::new(0.6, 0.6, 0.0),
                viewport_to_world(pos_vp.1, cam.0, cam.1).unwrap_or_default()
                    + Vec3::new(-0.6, 0.6, 0.0),
            );
            pos_ws
        } else {
            let pos_vp = (view_port.min, Vec2::new(view_port.min.x, view_port.max.y));
            let pos_ws = (
                viewport_to_world(pos_vp.0, cam.0, cam.1).unwrap_or_default()
                    + Vec3::new(0.6, -0.6, 0.0),
                viewport_to_world(pos_vp.1, cam.0, cam.1).unwrap_or_default()
                    + Vec3::new(0.6, 0.6, 0.0),
            );
            pos_ws
        };

        info!("start: {start}, end: {end}");

        for (i, mov) in solution.0.clone().into_iter().enumerate() {
            let end_idx = solution.0.total() - 1;
            let pos = start.lerp(end, i as f32 / end_idx as f32);
            painter.set_translation(pos);
            painter.set_color(Color::WHITE);
            painter.circle(0.07);
            if i >= solution.0.len() {
                painter.set_color(Color::BLACK);
                painter.circle(0.07 * 0.9);
            }
        }
    }
}

#[derive(Resource)]
struct Solution(solitaire_solver::Solution);

#[derive(Event)]
struct UpdateStats;

fn update_next_move_chance(
    _: Trigger<UpdateStats>,
    next_move_text: Query<Entity, With<NextMoveChanceText>>,
    board: Query<&BoardComponent>,
    feasible: Option<Res<FeasibleConstellations>>,
    mut writer: TextUiWriter,
    mut request_redraw: EventWriter<RequestRedraw>,
) {
    let Some(feasible) = feasible else {
        return;
    };
    let feasible = &feasible.0;
    let board = board.single().expect("board").board;
    let possible_moves = board.get_legal_moves();
    let correct_moves = possible_moves
        .iter()
        .copied()
        .filter(|m| feasible.contains(&board.mov(*m).normalize()))
        .collect::<Vec<_>>();
    let possible_moves = possible_moves.len();
    let correct_moves = correct_moves.len();
    for text in next_move_text {
        *writer.text(text, 1) = format!("{correct_moves} / {possible_moves}\n");
    }
    request_redraw.write(RequestRedraw);
}

#[derive(Event)]
struct ToggleHints;

#[derive(Resource)]
struct ShowHints;

fn toggle_hints(
    _: Trigger<ToggleHints>,
    mut commands: Commands,
    show_hints: Option<Res<ShowHints>>,
) {
    if show_hints.is_none() {
        commands.insert_resource(ShowHints);
    } else {
        commands.remove_resource::<ShowHints>();
    }
}

fn draw_possible_moves(
    mut painter: ShapePainter,
    board: Query<&BoardComponent>,
    feasible: Res<FeasibleConstellations>,
) {
    let feasible = &feasible.0;
    let board = board.single().expect("board").board;
    for y in 0..Board::SIZE {
        for x in 0..Board::SIZE {
            for dir in [Dir::North, Dir::East, Dir::South, Dir::West] {
                if !board.occupied((y, x)) {
                    continue;
                }
                if let Some(mov) = board.get_legal_move((y, x), dir) {
                    let start = board_to_world(BoardPosition {
                        x: mov.pos.1,
                        y: mov.pos.0,
                    });
                    let start = Vec3::from((start, MARKER_POS));
                    let target = board_to_world(BoardPosition {
                        x: mov.target.1,
                        y: mov.target.0,
                    });
                    let target = Vec3::from((target, MARKER_POS));
                    painter.set_color(if feasible.contains(&board.mov(mov).normalize()) {
                        Color::srgba(0., 1., 0., 1.)
                    } else {
                        Color::srgba(1., 0., 0., 1.)
                    });
                    painter.set_translation(Vec3::new(0., 0., 0.));
                    painter.thickness_type = ThicknessType::World;
                    painter.thickness = 0.075;
                    painter.line(start, start + (target - start) * 0.2);
                    painter.set_translation(start.xyz());
                    painter.circle(0.1);
                }
            }
        }
    }
}

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
    positions: &mut Query<(&mut BoardPosition, &mut Transform)>,
    follow_mouse: Query<&FollowMouse>,
    board: &mut Query<&mut BoardComponent>,
    cursor_pos: Vec2,
    camera_query: &Single<(&Camera, &GlobalTransform)>,
) {
    let (camera, camera_transform) = **camera_query;
    let Some(world_pos_cursor) = viewport_to_world(cursor_pos, camera, camera_transform) else {
        return;
    };
    let nearest_peg = world_to_board(world_pos_cursor.xy());
    if !Board::inbounds((nearest_peg.y, nearest_peg.x)) {
        commands.trigger(ToggleHints);
    }
    let mut board = board.single_mut().expect("board");
    for entity in pegs {
        if let Ok((mut board_pos, mut transform)) = positions.get_mut(entity) {
            let mut entity_commands = commands.entity(entity);
            if follow_mouse.contains(entity) {
                entity_commands.remove::<FollowMouse>();
                entity_commands.insert(SnapToBoardPosition);
                transform.translation.z = PEG_POS;

                // allow swapping pegs
                let current = (board_pos.y, board_pos.x);
                let destination = (nearest_peg.y, nearest_peg.x);
                if !Board::inbounds(destination) {
                    continue;
                }
                if board.board.occupied(destination) {
                    // *board_pos = nearest_peg;
                } else if let Some(mov) = board.board.is_legal_move(current, destination) {
                    println!("{mov}");
                    // update board
                    board.board = board.board.mov(mov);

                    // update peg position
                    let prev_pos = *board_pos;
                    let new_pos = BoardPosition {
                        y: destination.0,
                        x: destination.1,
                    };
                    *board_pos = new_pos;
                    commands.trigger(PegMoved {
                        prev_pos,
                        new_pos,
                        mov,
                    });
                    // remove skipped peg
                    for peg in pegs {
                        if let Ok((b, _)) = positions.get(peg) {
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
                    entity_commands.insert(FollowMouse);
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
    mut positions: Query<(&mut BoardPosition, &mut Transform)>,
    follow_mouse: Query<&FollowMouse>,
    mut board: Query<&mut BoardComponent>,
    camera_query: Single<(&Camera, &GlobalTransform)>,
) {
    if let Some(cursor_pos) = window.cursor_position() {
        handle_click(
            &mut commands,
            pegs,
            &mut positions,
            follow_mouse,
            &mut board,
            cursor_pos,
            &camera_query,
        )
    }
}

fn peg_selection_touch(
    mut commands: Commands,
    pegs: Query<Entity, With<Peg>>,
    mut positions: Query<(&mut BoardPosition, &mut Transform)>,
    follow_mouse: Query<&FollowMouse>,
    mut board: Query<&mut BoardComponent>,
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
                &mut positions,
                follow_mouse,
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
            let target = Vec3::from((board_to_world(*board_pos), PEG_POS));
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
    transforms: Query<&mut Transform, With<FollowMouse>>,
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
        app.insert_resource(Solution(Default::default()));
        app.add_observer(update_solution);
        app.add_systems(Update, draw_solution);
        app.add_systems(Update, toggle_fps_overlay);
        app.add_systems(Startup, (setup_board, spawn_pegs).chain());
        app.add_systems(Startup, setup_3d_meshes);
        // app.add_systems(Startup, camera_setup_3d);
        app.add_systems(Startup, (camera_setup, scale_viewport).chain());
        app.add_systems(Startup, create_solution_dag);
        app.add_systems(
            FixedUpdate,
            calculate_random_move_chances.run_if(resource_added::<FeasibleConstellations>),
        );
        app.add_systems(FixedUpdate, poll_task);
        app.insert_resource(ShowHints);
        app.add_observer(toggle_hints);
        app.add_systems(
            Update,
            draw_possible_moves.run_if(
                resource_exists::<ShowHints>.and(resource_exists::<FeasibleConstellations>),
            ),
        );
        app.add_systems(Update, snap_to_board_grid);
        app.add_systems(Update, follow_mouse);
        app.add_systems(
            Update,
            peg_selection_cursor.run_if(input_just_pressed(MouseButton::Left)),
        );
        app.add_systems(Startup, touch_hack);
        app.add_systems(Update, peg_selection_touch);
        app.add_observer(update_stats_on_move);
        app.add_systems(
            FixedUpdate,
            update_stats_on_solution.run_if(
                resource_added::<FeasibleConstellations>.or(resource_added::<RandomMoveChances>),
            ),
        );
        app.add_observer(update_next_move_chance);
        app.add_observer(update_overall_success);
        app.add_systems(Update, fullscreen_toggle);
        app.add_systems(Update, handle_exit);
        app.add_observer(update_window_theme);
        app.add_systems(Update, highlight_selected);
        app.add_systems(Startup, add_text);
    }
}

fn add_text(mut commands: Commands, asset_server: Res<AssetServer>) {
    let latin_modern = asset_server.load("fonts/latinmodern-math.otf");
    let large_font = TextFont {
        font: latin_modern.clone(),
        font_size: 100.0,
        ..default()
    };
    let medium_font = TextFont {
        font: latin_modern.clone(),
        font_size: 80.0,
        ..default()
    };
    let small_font = TextFont {
        font: latin_modern.clone(),
        font_size: 50.0,
        ..default()
    };
    let title_pos =
        Vec3::from((board_to_world(BoardPosition { x: 4, y: 4 }), 1.)) + Vec3::new(0.5, -0.5, 0.0);
    let title_pos_1 =
        Vec3::from((board_to_world(BoardPosition { x: 1, y: 4 }), 1.)) + Vec3::new(0.5, -0.5, 0.0);
    let text_pos = title_pos - 1.0 * Vec3::Y;
    commands
        .spawn((
            Text2d::new("\u{1D4AB}(\u{1D437}) \u{2248} "),
            Transform::from_scale(Vec3::new(0.005, 0.005, 0.005)).with_translation(title_pos),
            medium_font.clone(),
            TextLayout::new_with_justify(JustifyText::Left),
            Anchor::TopLeft,
            OverallSuccessRatio,
        ))
        .with_child((TextSpan(" ... ?".into()), medium_font.clone()));
    commands.spawn((
        Text2d::new("“chance of winning by chosing moves at random”"),
        Transform::from_scale(Vec3::new(0.004, 0.004, 0.004)).with_translation(text_pos),
        small_font.clone(),
        TextLayout::new(JustifyText::Center, LineBreak::WordBoundary),
        TextBounds::from(Vec2::new(600.0, 300.0)),
        Anchor::TopLeft,
    ));
    commands
        .spawn((
            Text2d::new(""),
            Transform::from_scale(Vec3::new(0.005, 0.005, 0.005)).with_translation(title_pos_1),
            large_font.clone(),
            TextLayout::new_with_justify(JustifyText::Center),
            Anchor::TopRight,
            NextMoveChanceText,
        ))
        .with_child((TextSpan("? / ?\n".into()), large_font.clone()))
        .with_child((
            TextSpan("moves lead to feasible\nconstellations".into()),
            small_font.clone(),
        ));
}

fn highlight_selected(mut painter: ShapePainter, selected: Query<&Transform, With<FollowMouse>>) {
    for selected in selected {
        painter.set_translation(selected.translation - Vec3::Z * 0.1);
        painter.set_color(Color::WHITE);
        painter.circle(PEG_RADIUS * 1.1);
    }
}

fn fullscreen_toggle(mut window: Single<&mut Window>, input: Res<ButtonInput<KeyCode>>) {
    if input.just_pressed(KeyCode::KeyF) {
        window.mode = match window.mode {
            WindowMode::Windowed => WindowMode::BorderlessFullscreen(MonitorSelection::Current),
            _ => WindowMode::Windowed,
        }
    }
}

fn handle_exit(input: Res<ButtonInput<KeyCode>>, mut exit: EventWriter<AppExit>) {
    if input.just_pressed(KeyCode::KeyQ) || input.all_just_pressed([KeyCode::AltLeft, KeyCode::F4])
    {
        exit.write(AppExit::Success);
    }
}

fn camera_transform_3d() -> Transform {
    Transform::from_xyz(6., 3., 10.).looking_at(Vec3::new(0.0, 0.0, 0.0), Vec3::Z)
}

fn board_to_world_transform() -> Transform {
    Transform::from_scale(Vec3::new(1., -1., 1.)).with_translation(Vec3::new(-3., 3., BOARD_POS))
    // Transform::from_translation(Vec3::new(-3., 3., 0.)).with_scale(Vec3::new(1., -1., 1.))
}

fn world_to_board_transform() -> Transform {
    Transform::from_matrix(board_to_world_transform().compute_matrix().inverse())
}

fn board_to_world(board_pos: BoardPosition) -> Vec2 {
    board_to_world_transform()
        .transform_point(Vec3::from((Vec2::from(board_pos), 0.)))
        .xy()
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

fn world_to_board(world_pos: Vec2) -> BoardPosition {
    let pos = world_to_board_transform().transform_point((world_pos, BOARD_POS).into());
    BoardPosition::from(pos.xy())
}

fn toggle_fps_overlay(input: Res<ButtonInput<KeyCode>>, mut overlay: ResMut<FpsOverlayConfig>) {
    if input.just_pressed(KeyCode::KeyD) {
        overlay.enabled = !overlay.enabled;
    }
}
