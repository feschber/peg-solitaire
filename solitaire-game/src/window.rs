use bevy::{
    log::{Level, LogPlugin},
    prelude::*,
    window::{WindowMode, WindowTheme, WindowThemeChanged},
};

pub struct MainWindow;

impl Plugin for MainWindow {
    fn build(&self, app: &mut App) {
        app.insert_resource(ClearColor(Color::BLACK)).add_plugins(
            DefaultPlugins
                .set(LogPlugin {
                    // This will show some log events from Bevy to the native logger.
                    level: Level::INFO,
                    filter: "wgpu=error,bevy_render=info,bevy_ecs=trace".to_string(),
                    ..Default::default()
                })
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        // title: "Peg Solitaire".into(),
                        fit_canvas_to_parent: true,
                        prevent_default_event_handling: false,
                        desired_maximum_frame_latency: core::num::NonZero::new(1u32),
                        present_mode: bevy::window::PresentMode::AutoVsync,
                        mode: WindowMode::BorderlessFullscreen(MonitorSelection::Primary),
                        // on iOS, gestures must be enabled.
                        // This doesn't work on Android
                        recognize_rotation_gesture: true,
                        // Only has an effect on iOS
                        prefers_home_indicator_hidden: true,
                        // Only has an effect on iOS
                        prefers_status_bar_hidden: true,
                        ..default()
                    }),
                    ..default()
                }),
        );
        app.add_systems(Update, handle_exit);
        app.add_systems(Update, fullscreen_toggle);
        app.add_observer(update_window_theme);
    }
}

fn update_window_theme(
    theme_changed: Trigger<WindowThemeChanged>,
    mut clear_color: ResMut<ClearColor>,
) {
    info!("Theme Changed!");
    match theme_changed.event().theme {
        WindowTheme::Light => *clear_color = ClearColor(Color::WHITE),
        WindowTheme::Dark => *clear_color = ClearColor(Color::BLACK),
    }
}

fn handle_exit(input: Res<ButtonInput<KeyCode>>, mut exit: EventWriter<AppExit>) {
    if input.just_pressed(KeyCode::KeyQ) || input.all_just_pressed([KeyCode::AltLeft, KeyCode::F4])
    {
        exit.write(AppExit::Success);
    }
}

fn fullscreen_toggle(mut window: Single<&mut Window>, input: Res<ButtonInput<KeyCode>>) {
    if input.just_pressed(KeyCode::KeyF) {
        window.mode = match window.mode {
            WindowMode::Windowed => WindowMode::BorderlessFullscreen(MonitorSelection::Current),
            _ => WindowMode::Windowed,
        }
    }
}
