use bevy::prelude::*;
use bevy_vector_shapes::prelude::*;

use crate::{
    BoardPosition, CurrentBoard,
    board::MARKER_POS,
    solver::{FeasibleConstellations, ForcedJumps},
};

pub struct HintsPlugin;

impl Plugin for HintsPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(Shape2dPlugin::default());
        app.add_observer(update_hints);
        app.add_systems(
            Update,
            (draw_possible_moves, draw_forced_jumps).run_if(
                resource_exists::<ShowHints>.and_then(resource_exists::<FeasibleConstellations>),
            ),
        );
    }
}

#[derive(Default, Event)]
pub struct ToggleHints;

#[derive(Resource)]
struct ShowHints;

fn update_hints(_: On<ToggleHints>, mut commands: Commands, show_hints: Option<Res<ShowHints>>) {
    if show_hints.is_none() {
        commands.insert_resource(ShowHints);
    } else {
        commands.remove_resource::<ShowHints>();
    }
}

/// Draws one marker per legal move: green if a win is still reachable after it, red if not.
///
/// Within the feasible set, a successor being feasible is exactly it being able to reach the
/// solved board, so this two-state colouring is already complete information about the moves
/// available *now* - including "only one of these is green", which is what makes a move
/// forced. Colouring that case specially was tried and dropped: it told the player nothing
/// they could not see by counting green lines. What they cannot see is
/// [`draw_forced_jumps`].
fn draw_possible_moves(
    mut painter: ShapePainter,
    board: Res<CurrentBoard>,
    feasible: Res<FeasibleConstellations>,
) {
    for mov in board.0.get_legal_moves() {
        let start = BoardPosition::from(mov.pos).to_world_space();
        let start = Vec3::from((start, MARKER_POS));
        let target = BoardPosition::from(mov.target).to_world_space();
        let target = Vec3::from((target, MARKER_POS));

        let winning = feasible.0.contains(&board.0.mov(mov).normalize());
        painter.set_color(if winning {
            Color::srgba(0., 1., 0., 1.)
        } else {
            Color::srgba(1., 0., 0., 1.)
        });
        painter.thickness_type = ThicknessType::World;
        painter.thickness = 0.075;
        painter.set_translation(Vec3::new(0., 0., 0.1));
        painter.line(start, start + (target - start) * 0.2);
        painter.set_translation(start.xyz());
        painter.circle(0.1);
    }
}

/// Ghosts the jumps the player is already committed to making later on.
///
/// This is the one thing the per-move colouring cannot show, because it is not about the
/// moves available now: these jumps are not legal yet, and may not be for another dozen
/// moves, but every winning continuation from the current position makes them. See
/// [`solitaire_solver::dominators::forced_jumps`] - and in particular why it works in the
/// player's own frame rather than the normalized quotient, which would claim the four
/// symmetric opening moves are one forced step.
///
/// Drawn unlike [`draw_possible_moves`]: the full span from origin to landing slot rather
/// than a stub, amber, and translucent, fading with how far off the jump is. Nothing is drawn
/// while the result is stale - it is recomputed off-thread on every move and near the opening
/// that takes seconds, so a result for a position already left has to be suppressed rather
/// than shown late.
fn draw_forced_jumps(
    mut painter: ShapePainter,
    board: Res<CurrentBoard>,
    forced: Option<Res<ForcedJumps>>,
) {
    let Some(forced) = forced else {
        return;
    };
    if forced.board != board.0 {
        return;
    }

    let pegs = board.0.count_pegs();
    for jump in &forced.jumps {
        // a forced jump *out of the current board* is exactly the single-green-line case, and
        // `draw_possible_moves` already draws it
        let ahead = pegs.saturating_sub(jump.board.count_pegs());
        if ahead == 0 {
            continue;
        }

        let start = BoardPosition::from(jump.mov.pos).to_world_space();
        let start = Vec3::from((start, MARKER_POS));
        let target = BoardPosition::from(jump.mov.target).to_world_space();
        let target = Vec3::from((target, MARKER_POS));

        // the soonest forced jump reads strongest; twenty moves out is barely there
        let fade = 1.0 - (ahead as f32 / 20.0).clamp(0.0, 1.0);
        let alpha = 0.15f32.lerp(0.7, fade);
        painter.set_color(Color::srgba(1.0, 0.72, 0.15, alpha));
        painter.thickness_type = ThicknessType::World;
        painter.thickness = 0.05;
        painter.set_translation(Vec3::new(0., 0., 0.1));
        painter.line(start, target);
        painter.set_translation(target.xyz());
        painter.circle(0.06);
    }
}
