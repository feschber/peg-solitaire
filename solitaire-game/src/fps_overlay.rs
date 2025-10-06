use bevy::{
    dev_tools::fps_overlay::{FpsOverlayConfig, FpsOverlayPlugin, FrameTimeGraphConfig},
    prelude::*,
};

pub struct FpsOverlay;

impl Plugin for FpsOverlay {
    fn build(&self, app: &mut App) {
        app.add_plugins(FpsOverlayPlugin {
            config: FpsOverlayConfig {
                frame_time_graph_config: FrameTimeGraphConfig {
                    enabled: true,
                    min_fps: 0.0,
                    target_fps: 120.0,
                },
                text_config: TextFont {
                    font_size: 10.0,
                    ..default()
                },
                text_color: Color::WHITE,
                refresh_interval: core::time::Duration::from_millis(100),
                enabled: false,
            },
        });
        app.add_systems(Update, toggle_fps_overlay);
    }
}

fn toggle_fps_overlay(input: Res<ButtonInput<KeyCode>>, mut overlay: ResMut<FpsOverlayConfig>) {
    if input.just_pressed(KeyCode::KeyD) {
        overlay.enabled = !overlay.enabled;
    }
}
