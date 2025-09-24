use bevy::prelude::*;
use bevy_vector_shapes::prelude::*;
use solitaire_solver::{Board, Dir};

use crate::{BoardPosition, CurrentBoard, board::MARKER_POS, solver::FeasibleConstellations};

pub struct HintsPlugin;

impl Plugin for HintsPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(ShowHints);
        app.add_plugins(Shape2dPlugin::default());
        app.add_observer(update_hints);
        app.add_systems(
            Update,
            draw_possible_moves.run_if(
                resource_exists::<ShowHints>.and(resource_exists::<FeasibleConstellations>),
            ),
        );
    }
}

#[derive(Default, Event)]
pub struct ToggleHints;

#[derive(Resource)]
struct ShowHints;

fn update_hints(
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
    board: Res<CurrentBoard>,
    feasible: Res<FeasibleConstellations>,
) {
    let feasible = &feasible.0;
    for y in 0..Board::SIZE {
        for x in 0..Board::SIZE {
            for dir in [Dir::North, Dir::East, Dir::South, Dir::West] {
                if !board.0.occupied((y, x)) {
                    continue;
                }
                if let Some(mov) = board.0.get_legal_move((y, x), dir) {
                    let start = BoardPosition::from(mov.pos).to_world_space();
                    let start = Vec3::from((start, MARKER_POS));
                    let target = BoardPosition::from(mov.target).to_world_space();
                    let target = Vec3::from((target, MARKER_POS));
                    painter.set_color(if feasible.contains(&board.0.mov(mov).normalize()) {
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
