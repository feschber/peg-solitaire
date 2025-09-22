use bevy::{prelude::*, sprite::Anchor, text::TextBounds, window::RequestRedraw};

use crate::{
    BoardPosition, CurrentBoard, PegMoved,
    solver::{FeasibleConstellations, RandomMoveChances},
};

pub struct StatsPlugin;

impl Plugin for StatsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, add_text);
        app.add_observer(update_stats_on_move);
        app.add_systems(
            FixedUpdate,
            update_stats_on_solution.run_if(
                resource_added::<FeasibleConstellations>.or(resource_added::<RandomMoveChances>),
            ),
        );
        app.add_observer(update_next_move_chance);
        app.add_observer(update_overall_success);
    }
}

#[derive(Event)]
struct UpdateStats;

#[derive(Component)]
struct NextMoveChanceText;

#[derive(Component)]
struct OverallSuccessRatio;

fn update_stats_on_solution(mut commands: Commands) {
    commands.trigger(UpdateStats);
}

fn update_stats_on_move(_trigger: Trigger<PegMoved>, mut commands: Commands) {
    commands.trigger(UpdateStats);
}

fn add_text(mut commands: Commands, asset_server: Res<AssetServer>) {
    let latin_modern = asset_server.load("fonts/latinmodern-math.otf");
    let large_font = TextFont {
        font: latin_modern.clone(),
        font_size: 100.0,
        ..default()
    };
    let medium_font = TextFont {
        font: latin_modern.clone(),
        font_size: 80.0,
        ..default()
    };
    let small_font = TextFont {
        font: latin_modern.clone(),
        font_size: 50.0,
        ..default()
    };
    let title_pos =
        Vec3::from((BoardPosition { x: 4, y: 4 }.to_world_space(), 1.)) + Vec3::new(0.5, -0.5, 0.0);
    let title_pos_1 =
        Vec3::from((BoardPosition { x: 1, y: 4 }.to_world_space(), 1.)) + Vec3::new(0.5, -0.5, 0.0);
    let text_pos = title_pos - 1.0 * Vec3::Y;
    commands
        .spawn((
            Text2d::new("\u{1D4AB}(\u{1D437}) \u{2248} "),
            Transform::from_scale(Vec3::new(0.005, 0.005, 0.005)).with_translation(title_pos),
            medium_font.clone(),
            TextLayout::new_with_justify(JustifyText::Left),
            Anchor::TopLeft,
            OverallSuccessRatio,
        ))
        .with_child((TextSpan(" ... ?".into()), medium_font.clone()));
    commands.spawn((
        Text2d::new("“chance of winning by chosing moves at random”"),
        Transform::from_scale(Vec3::new(0.004, 0.004, 0.004)).with_translation(text_pos),
        small_font.clone(),
        TextLayout::new(JustifyText::Center, LineBreak::WordBoundary),
        TextBounds::from(Vec2::new(600.0, 300.0)),
        Anchor::TopLeft,
    ));
    commands
        .spawn((
            Text2d::new(""),
            Transform::from_scale(Vec3::new(0.005, 0.005, 0.005)).with_translation(title_pos_1),
            large_font.clone(),
            TextLayout::new_with_justify(JustifyText::Center),
            Anchor::TopRight,
            NextMoveChanceText,
        ))
        .with_child((TextSpan("? / ?\n".into()), large_font.clone()))
        .with_child((
            TextSpan("moves lead to feasible\nconstellations".into()),
            small_font.clone(),
        ));
}

fn update_overall_success(
    _trigger: Trigger<UpdateStats>,
    overall_success_text: Query<Entity, With<OverallSuccessRatio>>,
    board: Res<CurrentBoard>,
    p_success: Option<Res<RandomMoveChances>>,
    mut writer: TextUiWriter,
    mut request_redraw: EventWriter<RequestRedraw>,
) {
    let Some(p_success) = p_success else {
        return;
    };
    let p_success = &p_success.0;
    let board = board.0.normalize();

    let p_success = *p_success.get(&board).unwrap_or(&0.0);
    // let p = num_rational::BigRational::from_float(p_success).unwrap();
    // let odds = p.clone() / (num_rational::BigRational::from_float(1.0).unwrap() - p.clone());
    for text in overall_success_text {
        if p_success > 0. {
            let inverse = 1. / p_success;
            let mut str = format!("1/{inverse:.0}");
            if str.len() > 4 {
                str = format!("\n{str}");
            }
            *writer.text(text, 1) = str;
        } else {
            *writer.text(text, 1) = format!("0");
        }
    }
    request_redraw.write(RequestRedraw);
}

fn update_next_move_chance(
    _: Trigger<UpdateStats>,
    next_move_text: Query<Entity, With<NextMoveChanceText>>,
    board: Res<CurrentBoard>,
    feasible: Option<Res<FeasibleConstellations>>,
    mut writer: TextUiWriter,
    mut request_redraw: EventWriter<RequestRedraw>,
) {
    let Some(feasible) = feasible else {
        return;
    };
    let feasible = &feasible.0;
    let possible_moves = board.0.get_legal_moves();
    let correct_moves = possible_moves
        .iter()
        .copied()
        .filter(|m| feasible.contains(&board.0.mov(*m).normalize()))
        .collect::<Vec<_>>();
    let possible_moves = possible_moves.len();
    let correct_moves = correct_moves.len();
    for text in next_move_text {
        *writer.text(text, 1) = format!("{correct_moves} / {possible_moves}\n");
    }
    request_redraw.write(RequestRedraw);
}
