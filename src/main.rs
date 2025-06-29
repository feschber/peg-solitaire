use bevy::prelude::*;

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_systems(Startup, spawn_pegs)
        .add_systems(Update, print_position_system)
        .run();
}

#[derive(Component)]
struct Position {
    x: i64,
    y: i64,
}

const BOARD_SIZE: i64 = 7;

fn spawn_pegs(mut commands: Commands) {
    for y in 0..BOARD_SIZE {
        for x in 0..BOARD_SIZE {
            commands.spawn_batch([(Name::new("peg"), Position { y, x })])
        }
    }
}

fn print_position_system(query: Query<&Position>) {
    for position in &query {
        println!("position: {} {}", position.x, position.y);
    }
}

struct Entity(u64);
