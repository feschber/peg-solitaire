use bevy::{
    ecs::entity_disabling::Disabled, prelude::*, sprite::Anchor, text::TextBounds,
    window::RequestRedraw,
};

use crate::{
    BoardPosition, CurrentBoard,
    solver::{FeasibleConstellations, RandomMoveChances},
    total_progress::TotalProgress,
};

#[derive(Default, Event)]
pub struct ToggleStats;

#[derive(Resource)]
struct ShowStats;

fn toggle_stats(
    _: On<ToggleStats>,
    mut commands: Commands,
    show_stats: Option<Res<ShowStats>>,
    stats: Query<
        Entity,
        (
            Or<(
                With<SolutionText>,
                With<OverallSuccessRatioText>,
                With<TotalProgressText>,
                With<NextMoveChanceText>,
            )>,
            Or<(With<Disabled>, Without<Disabled>)>,
        ),
    >,
) {
    if show_stats.is_none() {
        info!("Hiding Stats");
        commands.insert_resource(ShowStats);
        let mut i = 0;
        for e in &stats {
            info!("Hiding Stats ({i})");
            i += 1;
            let mut e = commands.entity(e);
            e.insert(Disabled);
        }
    } else {
        info!("Showing Stats");
        commands.remove_resource::<ShowStats>();
        let mut i = 0;
        for e in &stats {
            info!("Showing Stats ({i})");
            i += 1;
            let mut e = commands.entity(e);
            e.remove::<Disabled>();
        }
        commands.trigger(UpdateStats);
    }
}

pub struct StatsPlugin;

impl Plugin for StatsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, add_text);
        app.add_systems(
            Update,
            update_stats.run_if(
                resource_added::<FeasibleConstellations>
                    .or(resource_added::<RandomMoveChances>)
                    .or(resource_changed::<CurrentBoard>),
            ),
        );
        app.add_observer(update_next_move_chance);
        app.add_observer(update_overall_success);
        app.add_observer(update_total_progress);
        app.add_observer(update_solution_count);
        app.add_observer(toggle_stats);
    }
}

#[derive(Event)]
pub struct UpdateStats;

#[derive(Component)]
struct NextMoveChanceText;

#[derive(Component)]
struct TotalProgressText;

#[derive(Component)]
struct SolutionText;

#[derive(Component)]
struct OverallSuccessRatioText;

fn update_stats(mut commands: Commands) {
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
    let title_pos_2 =
        Vec3::from((BoardPosition { x: 4, y: 1 }.to_world_space(), 1.)) + Vec3::new(0.5, -0.5, 0.0);
    let title_pos_3 =
        Vec3::from((BoardPosition { x: 1, y: 1 }.to_world_space(), 1.)) + Vec3::new(0.5, -0.5, 0.0);
    commands
        .spawn((
            Text2d::new("\u{1D4AB}(\u{1D437}) \u{2248} "),
            Transform::from_scale(Vec3::new(0.005, 0.005, 0.005)).with_translation(title_pos),
            medium_font.clone(),
            TextLayout::new_with_justify(Justify::Left),
            Anchor::TOP_LEFT,
            OverallSuccessRatioText,
        ))
        .with_child((TextSpan(" ... ?".into()), medium_font.clone()))
        .with_child((
            TextSpan("\n“chance of winning by\nchosing moves at random”".into()),
            small_font.clone(),
        ));
    commands
        .spawn((
            Text2d::new(""),
            Transform::from_scale(Vec3::new(0.005, 0.005, 0.005)).with_translation(title_pos_1),
            large_font.clone(),
            TextLayout::new_with_justify(Justify::Center),
            Anchor::TOP_RIGHT,
            NextMoveChanceText,
        ))
        .with_child((TextSpan("? / ?\n".into()), large_font.clone()))
        .with_child((
            TextSpan("moves lead to feasible\nconstellations".into()),
            small_font.clone(),
        ));
    commands
        .spawn((
            Text2d::new("you have seen "),
            Transform::from_scale(Vec3::new(0.004, 0.004, 0.004)).with_translation(title_pos_2),
            small_font.clone(),
            TextLayout::new(Justify::Center, LineBreak::WordBoundary),
            TextBounds::from(Vec2::new(600.0, 300.0)),
            Anchor::BOTTOM_LEFT,
            TotalProgressText,
        ))
        .with_child((TextSpan("?%".into()), large_font.clone()))
        .with_child((
            TextSpan(" of feasible constellations".into()),
            small_font.clone(),
        ))
        .with_child((TextSpan("".into()), small_font.clone()));
    commands
        .spawn((
            Text2d::new("you have found "),
            Transform::from_scale(Vec3::new(0.004, 0.004, 0.004)).with_translation(title_pos_3),
            small_font.clone(),
            TextLayout::new(Justify::Center, LineBreak::WordBoundary),
            TextBounds::from(Vec2::new(600.0, 300.0)),
            Anchor::BOTTOM_RIGHT,
            SolutionText,
        ))
        .with_child((TextSpan(" ? ".into()), large_font.clone()))
        .with_child((TextSpan(" solutions, ".into()), small_font.clone()))
        .with_child((TextSpan(" ? ".into()), large_font.clone()))
        .with_child((TextSpan(" of which are unique!".into()), small_font.clone()));
}

fn update_overall_success(
    _trigger: On<UpdateStats>,
    overall_success_text: Query<Entity, With<OverallSuccessRatioText>>,
    board: Res<CurrentBoard>,
    p_success: Option<Res<RandomMoveChances>>,
    mut writer: TextUiWriter,
    mut request_redraw: MessageWriter<RequestRedraw>,
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
            *writer.text(text, 1) = "0".to_string();
        }
    }
    request_redraw.write(RequestRedraw);
}

fn update_next_move_chance(
    _: On<UpdateStats>,
    next_move_text: Query<Entity, With<NextMoveChanceText>>,
    board: Res<CurrentBoard>,
    feasible: Option<Res<FeasibleConstellations>>,
    mut writer: TextUiWriter,
    mut request_redraw: MessageWriter<RequestRedraw>,
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

fn update_total_progress(
    _: On<UpdateStats>,
    total_progress: Res<TotalProgress>,
    total_progress_text: Query<Entity, With<TotalProgressText>>,
    feasible_constellations: Option<Res<FeasibleConstellations>>,
    mut writer: TextUiWriter,
    mut request_redraw: MessageWriter<RequestRedraw>,
) {
    if let Some(feasible_constellations) = feasible_constellations {
        let progressed = total_progress.explored_states.len();
        let feasible = feasible_constellations.0.len();
        let explored = progressed as f64 / feasible as f64;
        let explored_perc = explored * 100.0;
        for text in total_progress_text {
            *writer.text(text, 1) = format!("{explored_perc:.3}%");
            *writer.text(text, 3) = format!(" ({progressed}/{feasible})");
        }
    }
    request_redraw.write(RequestRedraw);
}

fn update_solution_count(
    _: On<UpdateStats>,
    total_progress: Res<TotalProgress>,
    solution_text: Query<Entity, With<SolutionText>>,
    mut writer: TextUiWriter,
    mut request_redraw: MessageWriter<RequestRedraw>,
) {
    let num_solutions = total_progress.num_solutions;
    let num_unique_solutions = total_progress.unique_solutions.len();
    for text in solution_text {
        *writer.text(text, 1) = format!("{num_solutions}");
        *writer.text(text, 3) = format!("{num_unique_solutions}");
    }
    request_redraw.write(RequestRedraw);
}
