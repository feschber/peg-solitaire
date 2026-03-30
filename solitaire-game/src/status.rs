use bevy::{prelude::*, sprite::Anchor};
use bevy_vector_shapes::{prelude::ShapePainter, shapes::DiscPainter};

use crate::{CurrentSolution, viewport_to_world};

pub struct StatusPlugin;

impl Plugin for StatusPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, init_text);
        app.add_systems(Update, update_text);
        app.add_systems(Update, update_text_pos);
        app.add_systems(Update, draw_solution);
    }
}

#[derive(Component)]
struct MoveText(usize);

fn init_text(mut commands: Commands,
    solution: Res<CurrentSolution>,
    asset_server: Res<AssetServer>
) {
    let latin_modern = asset_server.load("fonts/latinmodern-math.otf");
    let small_font = TextFont {
        font: latin_modern.clone(),
        font_size: 50.0,
        ..default()
    };
    for i in 0..31 {
        commands
            .spawn((
                Text2d::new(""),
                Transform::from_scale(Vec3::new(0.005, 0.005, 0.005)),
                small_font.clone(),
                TextLayout::new_with_justify(Justify::Center),
                TextColor::WHITE,
                Anchor::CENTER,
                MoveText(i),
            ));
    }
}

fn update_text(
    moves: Query<(&mut Text2d, &MoveText)>,
    solution: Res<CurrentSolution>,
) {
    for (mut t, m) in moves {
        if m.0 < solution.0.len() {
            t.0 =  format!("{}", solution.0[m.0]);
        } else {
            t.0 = "".into()
        }
    }
}

fn update_text_pos(
    moves: Query<(&mut Transform, &MoveText)>,
    solution: Res<CurrentSolution>,
    camera_query: Single<(&Camera, &GlobalTransform)>,
) {
    let (cam, gt) = &*camera_query;
    if let Some(view_port) = cam.logical_viewport_rect() {
        for (mut t, m) in moves {
            let pos = pos(cam, gt, view_port, m.0, &*solution);
            t.translation = pos + Vec3::Y * 0.5;
        }
    }
}

fn draw_solution(
    solution: Res<CurrentSolution>,
    mut painter: ShapePainter,
    camera_query: Single<(&Camera, &GlobalTransform)>,
) {
    let (cam, gt) = &*camera_query;
    if let Some(view_port) = cam.logical_viewport_rect() {
        for (i, _mov) in solution.0.clone().into_iter().enumerate() {
            let pos = pos(cam, gt, view_port, i, &*solution);
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

fn pos(cam: &Camera, global_transform: &GlobalTransform, view_port: Rect, i: usize, solution: &CurrentSolution) -> Vec3 {
        let pos_vp = (Vec2::new(view_port.min.x, view_port.max.y), view_port.max);
        let end_idx = solution.0.total() - 1;
        let (start, end) = (
            viewport_to_world(pos_vp.0, cam, global_transform).unwrap_or_default()
                + Vec3::new(0.6, 0.6, 0.0),
            viewport_to_world(pos_vp.1, cam, global_transform).unwrap_or_default()
                + Vec3::new(-0.6, 0.6, 0.0),
        );
        start.lerp(end, i as f32 / end_idx as f32)

}
