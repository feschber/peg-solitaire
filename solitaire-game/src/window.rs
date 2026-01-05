use bevy::{
    log::{Level, LogPlugin},
    prelude::*,
    window::{WindowMode, WindowTheme, WindowThemeChanged},
    winit::WinitSettings,
};

pub struct MainWindow;

impl Plugin for MainWindow {
    fn build(&self, app: &mut App) {
        app.insert_resource(WinitSettings::desktop_app());

        let default_plugins = DefaultPlugins
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
                    #[cfg(not(target_os = "android"))]
                    mode: WindowMode::Windowed,
                    #[cfg(target_os = "android")]
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
            });
        #[cfg(target_arch = "wasm32")]
        let default_plugins = default_plugins
            .set(AssetPlugin {
                meta_check: bevy::asset::AssetMetaCheck::Never,
                ..default()
            })
            .set(WindowPlugin {
                primary_window: Some(Window {
                    title: "I am a window!".into(),
                    name: Some("bevy.app".into()),
                    resolution: (500, 300).into(),
                    present_mode: bevy::window::PresentMode::AutoVsync,
                    // Tells Wasm to resize the window according to the available canvas
                    fit_canvas_to_parent: true,
                    // Tells Wasm not to override default event handling, like F5, Ctrl+R etc.
                    prevent_default_event_handling: false,
                    window_theme: Some(WindowTheme::Dark),
                    enabled_buttons: bevy::window::EnabledButtons {
                        maximize: false,
                        ..Default::default()
                    },
                    // This will spawn an invisible window
                    // The window will be made visible in the make_visible() system after 3 frames.
                    // This is useful when you want to avoid the white window that shows up before the GPU is ready to render the app.
                    visible: false,
                    ..default()
                }),
                ..default()
            });
        app.insert_resource(ClearColor(Color::BLACK));
        app.add_plugins(default_plugins);
        app.add_systems(Update, handle_exit);
        app.add_systems(Update, fullscreen_toggle);
        app.add_systems(Update, update_window_theme);
    }
}

fn update_window_theme(
    mut theme_changed: MessageReader<WindowThemeChanged>,
    mut clear_color: ResMut<ClearColor>,
) {
    for message in theme_changed.read() {
        info!("Theme Changed!");
        match message.theme {
            WindowTheme::Light => *clear_color = ClearColor(Color::WHITE),
            WindowTheme::Dark => *clear_color = ClearColor(Color::BLACK),
        }
    }
}

fn handle_exit(input: Res<ButtonInput<KeyCode>>, mut exit: MessageWriter<AppExit>) {
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

#[cfg(target_arch = "wasm32")]
fn make_visible(mut window: Single<&mut Window>, frames: Res<bevy::diagnostic::FrameCount>) {
    // The delay may be different for your app or system.
    if frames.0 == 3 {
        // At this point the gpu is ready to show the app so we can make the window visible.
        // Alternatively, you could toggle the visibility in Startup.
        // It will work, but it will have one white frame before it starts rendering
        window.visible = true;
    }
}
