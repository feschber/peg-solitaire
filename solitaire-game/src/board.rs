use bevy::{
    prelude::*,
    render::mesh::{CircleMeshBuilder, SphereKind, SphereMeshBuilder},
};
use solitaire_solver::Board;

use crate::CurrentBoard;

pub struct BoardPlugin;

impl Plugin for BoardPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_pegs);
    }
}

pub const BOARD_POS: f32 = 0.0;
pub const MARKER_POS: f32 = 0.1;
pub const PEG_POS: f32 = 0.2;
pub const PEG_POS_RAISED: f32 = 1.0;
pub const GOLDEN_RATIO: f32 = 1.618033988749;
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
    pub fn to_world_space(&self) -> Vec2 {
        board_to_world_transform()
            .transform_point(Vec3::from((Vec2::from(*self), 0.)))
            .xy()
    }
}

fn board_to_world_transform() -> Transform {
    Transform::from_scale(Vec3::new(1., -1., 1.)).with_translation(Vec3::new(-3., 3., BOARD_POS))
    // Transform::from_translation(Vec3::new(-3., 3., 0.)).with_scale(Vec3::new(1., -1., 1.))
}

fn world_to_board_transform() -> Transform {
    Transform::from_matrix(board_to_world_transform().compute_matrix().inverse())
}

fn spawn_pegs(
    mut commands: Commands,
    board: Res<CurrentBoard>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut color_materials: ResMut<Assets<ColorMaterial>>,
) {
    // the board itself
    commands.spawn((
        BoardMarker,
        Name::new("board"),
        Transform::from_translation(Vec3::new(0., 0., BOARD_POS)),
        Mesh2d(meshes.add(CircleMeshBuilder::new(3.9, 1000).build())),
        MeshMaterial2d(color_materials.add(Color::WHITE.with_luminance(0.02))),
    ));

    let board = &board.0;
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
            let world_pos = board_pos.to_world_space();
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
