use bevy::{input::common_conditions::input_just_pressed, prelude::*, window::PrimaryWindow};
use bevy_vector_shapes::{
    Shape2dPlugin,
    prelude::ShapePainter,
    shapes::{DiscPainter, LinePainter, ThicknessType},
};
use solitaire_solver::{BOARD_SIZE, Board, Dir, Solution, SolutionDag};

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(Shape2dPlugin::default())
        .add_plugins(PegSolitaire)
        .run();
}

#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
struct BoardPosition {
    x: i64,
    y: i64,
}

#[derive(Component, Clone, Copy, Debug, PartialEq)]
struct Position {
    pos: Vec3,
}

#[derive(Component)]
struct ColorComp {
    col: Color,
}

#[derive(Component)]
struct BoardComponent {
    /// represents the currently active board
    board: Board,
}

#[derive(Component)]
struct SolutionComponent {
    solution: SolutionDag,
}

impl Default for BoardComponent {
    fn default() -> Self {
        let board = Board::default();
        BoardComponent { board }
    }
}

fn calculate_solution_dag(mut commands: Commands) {
    let mut solution_dag = SolutionDag::new(Board::default());
    let mut current = Solution::default();
    solitaire_solver::solve_all(Board::default(), &mut current, &mut solution_dag);
    commands.spawn(SolutionComponent {
        solution: solution_dag,
    });
}

fn board_to_screen_space(pos: BoardPosition, z: f32) -> Position {
    let offset = 2. * PEG_RADIUS as f32 + PEG_DIST as f32;
    let (x, y) = ((pos.x - 3) as f32, (pos.y - 3) as f32);
    let pos = Vec3::new(x * offset, -y * offset, z);
    Position { pos }
}

fn screen_to_board(pos: Position) -> BoardPosition {
    let offset = 2. * PEG_RADIUS as f32 + PEG_DIST as f32;
    let pos = pos.pos / offset;
    let (y, x) = (-pos.y.round() as i64, pos.x.round() as i64);
    let (y, x) = (y + 3, x + 3);
    BoardPosition { x, y }
}

fn setup_board(mut commands: Commands) {
    commands.spawn(BoardComponent::default());
}

#[derive(Component)]
struct BoardMarker;

fn spawn_pegs(mut commands: Commands, board: Query<&BoardComponent>) {
    // the board itself
    let board_radius = (PEG_RADIUS * 2 + PEG_DIST) * 4;
    commands.spawn((
        BoardMarker,
        Name::new("board"),
        Position {
            pos: Vec3::new(0., 0., -2.),
        },
        ColorComp {
            col: Color::WHITE.with_luminance(0.10),
        },
        CircleComponent {
            radius: board_radius,
        },
    ));

    let board = board.single().expect("board").board;
    for y in 0..BOARD_SIZE {
        for x in 0..BOARD_SIZE {
            let pos = (y, x);
            if Board::inbounds(pos) {
                // spawn holes
                commands.spawn((
                    board_to_screen_space(BoardPosition { y, x }, -1.),
                    ColorComp {
                        col: Color::WHITE.with_luminance(0.07),
                    },
                    CircleComponent {
                        radius: (PEG_RADIUS as f32 * 0.9) as i32,
                    },
                ));
            }

            // spawn pegs
            if board.occupied((y, x)) {
                let board_pos = BoardPosition { y, x };
                let position = board_to_screen_space(board_pos, 0.);
                commands.spawn((
                    board_pos,
                    position,
                    ColorComp {
                        col: Color::hsl(((y * 7 + x) * 16) as f32, 1., 0.7),
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

#[derive(Component)]
struct SnapToBoardPosition;

fn camera_setup(mut commands: Commands) {
    commands.spawn(Camera2d);
}

fn draw_circles(
    mut painter: ShapePainter,
    circles: Query<(&CircleComponent, &Position, &ColorComp)>,
) {
    for (circle, pos, color) in circles {
        let pos = pos.pos;
        painter.set_translation(pos);
        painter.set_color(color.col);
        painter.circle(circle.radius as f32);
    }
}

fn draw_possible_moves(
    mut painter: ShapePainter,
    board: Query<&BoardComponent>,
    solution_dag: Query<&SolutionComponent>,
) {
    let Ok(solution) = solution_dag.single() else {
        return;
    };
    let solution = &solution.solution;
    let board = board.single().expect("board").board;
    for y in 0..BOARD_SIZE {
        for x in 0..BOARD_SIZE {
            for dir in [Dir::North, Dir::East, Dir::South, Dir::West] {
                if !board.occupied((y, x)) {
                    continue;
                }
                if let Some(mov) = board.get_legal_move((y, x), dir) {
                    let start = board_to_screen_space(
                        BoardPosition {
                            x: mov.pos.1,
                            y: mov.pos.0,
                        },
                        2.,
                    );
                    let target = board_to_screen_space(
                        BoardPosition {
                            x: mov.target.1,
                            y: mov.target.0,
                        },
                        2.,
                    );
                    let new_board = board.mov(mov);
                    let solvable = solution.has_solution(new_board);
                    painter.set_color(if solvable {
                        Color::srgba(0., 1., 0., 1.)
                    } else {
                        Color::srgba(1., 0., 0., 1.)
                    });
                    painter.set_translation(Vec3::new(0., 0., 2.));
                    painter.thickness_type = ThicknessType::Pixels;
                    painter.thickness = 3.;
                    painter.line(start.pos, start.pos + (target.pos - start.pos) * 0.25);
                    painter.set_translation(start.pos);
                    painter.circle(PEG_RADIUS as f32 * 0.2);
                }
            }
        }
    }
}

fn peg_selection(
    mut commands: Commands,
    window: Single<&Window, With<PrimaryWindow>>,
    pegs: Query<Entity, With<Selectable>>,
    mut positions: Query<(&mut BoardPosition, &mut Position)>,
    follow_mouse: Query<&FollowMouse>,
    mut board: Query<&mut BoardComponent>,
    solution_graph: Query<&SolutionComponent>,
    mut board_background: Query<&mut ColorComp, With<BoardMarker>>,
) {
    if let Some(cursor_pos) = window.cursor_position() {
        let cursor_pos = cursor_to_screen_space(cursor_pos, window.size());
        let nearest_peg = screen_to_board(Position { pos: cursor_pos });
        let mut board = board.single_mut().expect("board");
        println!("mouse pos: {cursor_pos}");
        for entity in pegs {
            if let Ok((mut board_pos, mut position)) = positions.get_mut(entity) {
                let mut entity_commands = commands.entity(entity);
                if follow_mouse.contains(entity) {
                    entity_commands.remove::<FollowMouse>();
                    entity_commands.insert(SnapToBoardPosition);
                    position.pos.z = 1.;

                    // allow swapping pegs
                    let current = (board_pos.y, board_pos.x);
                    let destination = (nearest_peg.y, nearest_peg.x);
                    println!("{current:?} -> {destination:?}");
                    if board.board.occupied(destination) {
                        // *board_pos = nearest_peg;
                    } else if let Some(mov) = board.board.is_legal_move(current, destination) {
                        println!("{mov}");
                        // update board
                        board.board = board.board.mov(mov);
                        if let Ok(sol) = solution_graph.single() {
                            let solvable = sol.solution.has_solution(board.board);
                            if !solvable {
                                board_background.single_mut().unwrap().col =
                                    Color::srgb(1., 0., 0.);
                            }
                        }
                        // update peg position
                        board_pos.y = destination.0;
                        board_pos.x = destination.1;
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
                        position.pos.z = 1.;
                    }
                }
            }
        }
    }
}

fn snap_to_board_grid(
    mut commands: Commands,
    pegs: Query<Entity, With<SnapToBoardPosition>>,
    mut pos: Query<(&BoardPosition, &mut Position), With<SnapToBoardPosition>>,
) {
    for peg in pegs {
        if let Ok((board_pos, mut screen_pos)) = pos.get_mut(peg) {
            let target = board_to_screen_space(*board_pos, 1.);
            let new_pos = lerp(*screen_pos, target, 0.2);
            *screen_pos = new_pos;
            if new_pos == target {
                commands.entity(peg).remove::<SnapToBoardPosition>();
            }
        }
    }
}

fn lerp(a: Position, b: Position, s: f32) -> Position {
    Position {
        pos: a.pos.lerp(b.pos, s),
    }
}

fn follow_mouse(
    window: Single<&Window, With<PrimaryWindow>>,
    positions: Query<&mut Position, With<FollowMouse>>,
) {
    if let Some(cursor_pos) = window.cursor_position() {
        for mut pos in positions {
            let z = pos.pos.z;
            let mut destination = cursor_to_screen_space(cursor_pos, window.size());
            destination.z = z;
            let destination = Position { pos: destination };
            *pos = destination;
        }
    }
}

fn cursor_to_screen_space(cursor_pos: Vec2, window_size: Vec2) -> Vec3 {
    let pos = (cursor_pos - window_size / 2.) * (Vec2::X - Vec2::Y);
    Vec3::new(pos.x, pos.y, 0.)
}

struct PegSolitaire;

impl Plugin for PegSolitaire {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, (setup_board, spawn_pegs).chain());
        app.add_systems(Startup, camera_setup);
        app.add_systems(PostStartup, calculate_solution_dag);
        app.add_systems(Update, draw_circles);
        app.add_systems(Update, draw_possible_moves);
        app.add_systems(Update, snap_to_board_grid);
        app.add_systems(Update, follow_mouse);
        app.add_systems(
            Update,
            peg_selection
                .run_if(input_just_pressed(MouseButton::Left))
                .chain(),
        );
    }
}
