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
                    enabled: false,
                    min_fps: 0.0,
                    target_fps: 120.0,
                },
                text_config: TextFont {
                    font_size: FontSize::Px(10.0),
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

/// `F3`, not `D`: `D` is also strafe-right in both graph cameras (see `graph.rs`), so
/// panning around the graph used to toggle the overlay on every keypress - which matters
/// because this readout is the only frame-time instrument the app has.
fn toggle_fps_overlay(input: Res<ButtonInput<KeyCode>>, mut overlay: ResMut<FpsOverlayConfig>) {
    if input.just_pressed(KeyCode::F3) {
        overlay.enabled = !overlay.enabled;
    }
}
