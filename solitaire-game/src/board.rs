use bevy::{ecs::entity_disabling::Disabled, prelude::*};
use bevy_vector_shapes::{prelude::ShapePainter, shapes::DiscPainter};
use solitaire_solver::Board;

use crate::{CurrentBoard, MoveEvent, PegMoved, input::RequestPegMove};

pub struct BoardPlugin;

impl Plugin for BoardPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_pegs);
        app.add_observer(on_peg_move_request);
        app.add_observer(on_move_peg);
        app.add_systems(Update, draw_pegs);
    }
}

pub const BOARD_POS: f32 = 0.0;
pub const MARKER_POS: f32 = 0.1;
pub const PEG_POS: f32 = 0.2;
pub const PEG_POS_RAISED: f32 = 1.0;
pub const GOLDEN_RATIO: f32 = 1.618_034;
pub const PEG_RADIUS: f32 = 1. / (2. * GOLDEN_RATIO);
pub const HOLE_RADIUS: f32 = 0.9 * PEG_RADIUS;

#[derive(Component)]
struct BoardMarker;

#[derive(Component)]
pub struct Peg;

#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct BoardPosition {
    pub x: i64,
    pub y: i64,
}

#[derive(Event)]
struct MovePeg {
    mov: solitaire_solver::Move,
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

impl From<(i64, i64)> for BoardPosition {
    fn from(value: (i64, i64)) -> Self {
        let (y, x) = value;
        Self { y, x }
    }
}

impl From<BoardPosition> for (i64, i64) {
    fn from(pos: BoardPosition) -> Self {
        (pos.y, pos.x)
    }
}

impl From<&BoardPosition> for (i64, i64) {
    fn from(value: &BoardPosition) -> Self {
        (*value).into()
    }
}

impl From<&mut BoardPosition> for (i64, i64) {
    fn from(value: &mut BoardPosition) -> Self {
        (*value).into()
    }
}

impl BoardPosition {
    pub fn from_world_space(world_pos: Vec2) -> BoardPosition {
        let pos = world_to_board_transform().transform_point((world_pos, BOARD_POS).into());
        BoardPosition::from(pos.xy())
    }
    pub fn to_world_space(self) -> Vec2 {
        board_to_world_transform()
            .transform_point(Vec3::from((Vec2::from(self), 0.)))
            .xy()
    }
}

fn board_to_world_transform() -> Transform {
    Transform::from_scale(Vec3::new(1., -1., 1.)).with_translation(Vec3::new(-3., 3., BOARD_POS))
    // Transform::from_translation(Vec3::new(-3., 3., 0.)).with_scale(Vec3::new(1., -1., 1.))
}

fn world_to_board_transform() -> Transform {
    Transform::from_matrix(board_to_world_transform().to_matrix().inverse())
}

#[derive(Component)]
struct CircleComponent {
    radius: f32,
    color: Color,
}

fn spawn_pegs(mut commands: Commands, board: Res<CurrentBoard>) {
    // the board itself
    commands.spawn((
        BoardMarker,
        Transform::from_translation(Vec3::new(0., 0., BOARD_POS)),
        CircleComponent {
            radius: 3.9,
            color: Color::WHITE.with_luminance(0.02),
        },
    ));

    let board = &board.0;
    for y in 0..Board::SIZE {
        for x in 0..Board::SIZE {
            let board_pos = BoardPosition { y, x };
            let world_pos = board_pos.to_world_space();
            if Board::inbounds((y, x)) {
                // spawn holes
                commands.spawn((
                    CircleComponent {
                        radius: HOLE_RADIUS,
                        color: Color::WHITE.with_luminance(0.01),
                    },
                    Transform::from_translation((world_pos, BOARD_POS).into()),
                ));
            }

            // spawn pegs
            let color = Color::hsl(((y * 7 + x) * 16) as f32, 1., 0.7);
            if board.occupied((y, x)) {
                commands.spawn((
                    CircleComponent {
                        radius: PEG_RADIUS,
                        color,
                    },
                    BoardPosition { y, x },
                    Transform::from_translation((world_pos, PEG_POS).into()),
                    Peg,
                ));
            }
        }
    }
}

fn draw_pegs(mut painter: ShapePainter, circles: Query<(&Transform, &CircleComponent)>) {
    for (transform, circle) in circles {
        painter.transform = *transform;
        painter.set_color(circle.color);
        painter.circle(circle.radius);
    }
}

/// request to move peg comming from input system
fn on_peg_move_request(
    move_request: On<RequestPegMove>,
    mut board: ResMut<CurrentBoard>,
    mut commands: Commands,
) {
    let src = move_request.src;
    let dst = move_request.dst;
    if let Some(mov) = board.0.is_legal_move(src.into(), dst.into()) {
        board.0 = board.0.mov(mov);
        commands.trigger(MovePeg { mov });
    }
}

fn on_move_peg(
    move_peg: On<MovePeg>,
    mut pegs: Query<(Entity, &mut BoardPosition), With<Peg>>,
    mut commands: Commands,
) {
    let mov = move_peg.mov;
    let prev_pos: BoardPosition = mov.pos.into();
    let skipped_pos: BoardPosition = mov.skip.into();
    let new_pos: BoardPosition = mov.target.into();
    let (skipped, _) = pegs
        .iter()
        .find(|(_, p)| **p == skipped_pos)
        .expect("skipped");
    // move peg
    let (moved, mut p) = pegs.iter_mut().find(|(_, p)| **p == prev_pos).expect("peg");
    *p = new_pos;
    // disable skipped peg
    commands.entity(skipped).insert(Disabled);

    // trigger moved event
    commands.trigger(MoveEvent {
        mov,
        moved,
        skipped,
    });
    commands.trigger(PegMoved { peg: moved });
}
