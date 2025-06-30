use bevy::{input::common_conditions::input_just_pressed, prelude::*, window::PrimaryWindow};
use bevy_vector_shapes::{Shape2dPlugin, prelude::ShapePainter, shapes::DiscPainter};

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(Shape2dPlugin::default())
        .add_plugins(Board)
        .run();
}

#[derive(Component)]
struct BoardPosition {
    x: i64,
    y: i64,
}

#[derive(Component)]
struct Position {
    pos: Vec2,
}

#[derive(Component)]
struct ColorComp {
    col: Color,
}

const BOARD_SIZE: i64 = 7;

fn board_to_screen_space(pos: BoardPosition) -> Position {
    let offset = 2. * PEG_RADIUS as f32 + PEG_DIST as f32;
    let (x, y) = ((pos.x - 3) as f32, (pos.y - 3) as f32);
    let pos = Vec2::new(x * offset, y * offset);
    Position { pos }
}

fn spawn_pegs(mut commands: Commands) {
    // board itself
    let board_radius = (PEG_RADIUS * 2 + PEG_DIST) * 4;
    commands.spawn((
        Name::new("board"),
        BoardPosition { y: 3, x: 3 },
        ColorComp {
            col: Color::WHITE.with_luminance(0.3),
        },
        CircleComponent {
            radius: board_radius,
        },
    ));

    for y in 0..BOARD_SIZE {
        for x in 0..BOARD_SIZE {
            if inbounds((y, x)) {
                // spawn holes
                commands.spawn((
                    Name::new("peg"),
                    board_to_screen_space(BoardPosition { y, x }),
                    ColorComp {
                        col: Color::WHITE.with_luminance(0.2),
                    },
                    CircleComponent {
                        radius: (PEG_RADIUS as f32 * 0.9) as i32,
                    },
                ));

                // spawn pegs
                if y == 3 && x == 3 {
                    continue;
                }
                commands.spawn((
                    Name::new("peg"),
                    board_to_screen_space(BoardPosition { y, x }),
                    ColorComp {
                        col: Color::hsl(((y * 7 + x) * 16) as f32, 1., 0.5),
                    },
                    Selectable,
                    CircleComponent { radius: PEG_RADIUS },
                ));
            }
        }
    }
}

#[derive(Component)]
struct Camera;

const GOLDEN_RATIO: f32 = 1.618033988749;

const PEG_RADIUS: i32 = 30;
const PEG_DIST: i32 = (PEG_RADIUS as f32 * GOLDEN_RATIO - PEG_RADIUS as f32) as i32 * 2;

#[derive(Component)]
struct CircleComponent {
    radius: i32,
}

#[derive(Component)]
struct FollowMouse;

#[derive(Component)]
struct Selectable;

fn camera_setup(mut commands: Commands) {
    commands.spawn(Camera2d);
}

fn draw_circles(
    mut painter: ShapePainter,
    circles: Query<(&CircleComponent, &Position, &ColorComp)>,
) {
    for (circle, pos, color) in circles {
        let pos = pos.pos;
        painter.set_translation(Vec3::new(pos.x, pos.y, 0.));
        painter.set_color(color.col);
        painter.circle(circle.radius as f32);
    }
}

fn peg_selection(
    mut commands: Commands,
    window: Single<&Window, With<PrimaryWindow>>,
    pegs: Query<Entity, With<Selectable>>,
    positions: Query<(&Position, &CircleComponent)>,
) {
    if let Some(cursor_pos) = window.cursor_position() {
        let cursor_pos = cursor_to_screen_space(cursor_pos, window.size());
        println!("mouse pos: {cursor_pos}");
        for entity in pegs {
            if let Ok((position, circle)) = positions.get(entity) {
                let distance = cursor_pos.distance(position.pos);
                if distance < circle.radius as f32 {
                    commands.entity(entity).insert(FollowMouse);
                }
            }
        }
    }
}

fn follow_mouse(
    window: Single<&Window, With<PrimaryWindow>>,
    positions: Query<&mut Position, With<FollowMouse>>,
) {
    if let Some(cursor_pos) = window.cursor_position() {
        for mut pos in positions {
            pos.pos = cursor_to_screen_space(cursor_pos, window.size());
        }
    }
}

fn cursor_to_screen_space(cursor_pos: Vec2, window_size: Vec2) -> Vec2 {
    (cursor_pos - window_size / 2.) * (Vec2::X - Vec2::Y)
}

struct Board;

impl Plugin for Board {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_pegs);
        app.add_systems(Startup, camera_setup);
        app.add_systems(Update, draw_circles);
        app.add_systems(
            Update,
            peg_selection.run_if(input_just_pressed(MouseButton::Left)),
        );
        app.add_systems(Update, follow_mouse);
    }
}

type Idx = i64;

#[inline(always)]
fn inbounds(pos: (Idx, Idx)) -> bool {
    let (y, x) = pos;
    in_mid_section(x) && in_whole_range(y) || in_mid_section(y) && in_whole_range(x)
}

#[inline(always)]
fn in_mid_section(i: Idx) -> bool {
    (2..5).contains(&i)
}

#[inline(always)]
fn in_whole_range(i: Idx) -> bool {
    (0..BOARD_SIZE).contains(&i)
}
