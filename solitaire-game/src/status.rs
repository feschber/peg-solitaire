use bevy::prelude::*;
use bevy_vector_shapes::{prelude::ShapePainter, shapes::DiscPainter};

use crate::{CurrentSolution, viewport_to_world};

pub struct StatusPlugin;

impl Plugin for StatusPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, draw_solution);
    }
}

fn draw_solution(
    solution: Res<CurrentSolution>,
    mut painter: ShapePainter,
    camera_query: Single<(&Camera, &GlobalTransform)>,
) {
    let cam = camera_query;
    if let Some(view_port) = cam.0.logical_viewport_rect() {
        let y_size = view_port.max.y - view_port.min.y;
        let x_size = view_port.max.x - view_port.min.x;

        let (start, end) = if y_size > x_size {
            let pos_vp = (Vec2::new(view_port.min.x, view_port.max.y), view_port.max);
            (
                viewport_to_world(pos_vp.0, cam.0, cam.1).unwrap_or_default()
                    + Vec3::new(0.6, 0.6, 0.0),
                viewport_to_world(pos_vp.1, cam.0, cam.1).unwrap_or_default()
                    + Vec3::new(-0.6, 0.6, 0.0),
            )
        } else {
            let pos_vp = (view_port.min, Vec2::new(view_port.min.x, view_port.max.y));
            (
                viewport_to_world(pos_vp.0, cam.0, cam.1).unwrap_or_default()
                    + Vec3::new(0.6, -0.6, 0.0),
                viewport_to_world(pos_vp.1, cam.0, cam.1).unwrap_or_default()
                    + Vec3::new(0.6, 0.6, 0.0),
            )
        };

        for (i, _mov) in solution.0.clone().into_iter().enumerate() {
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
