//! A 3d view of the feasible constellation graph.
//!
//! Every node is one feasible constellation - a board that lies on at least one
//! complete solution - and every edge is a legal move. Height is the peg count, so
//! all edges point downwards and the whole graph reads as a funnel from the widest
//! layer down to the single solved board at the apex.
//!
//! The solver hands out the feasible set as a flat `Vec<Board>` with no edges and no
//! layer index (see `solitaire_solver::calculate_feasible_set`), so both are derived
//! here. Node identity is the *normalized* board, i.e. one node per symmetry orbit,
//! which is what the solver stores and what `hints.rs` already looks up.
//!
//! Bounded to [`MAX_PEGS`] pegs. Measured feasible counts per layer, for sizing:
//!
//! | pegs  | 1 | 2 | 3 | 4  | 5  | 6   | 7   | 8    | 9    | 10    | 11    | 12    |
//! |-------|---|---|---|----|----|-----|-----|------|------|-------|-------|-------|
//! | nodes | 1 | 1 | 2 | 8  | 38 | 164 | 635 | 2089 | 6174 | 16020 | 35749 | 68326 |
//!
//! which is 129_207 nodes up to 12 pegs. The next layers are 112_788 / 162_319 /
//! 204_992 / 230_230, and the full graph is 1_679_072 nodes - see [`MAX_PEGS`].
//!
//! Toggled with the graph button or `G`. Left-drag orbits, right-drag pans, the wheel
//! zooms, and `WASD` + `space`/`shift` flies.

use bevy::{
    asset::RenderAssetUsages,
    camera::visibility::NoFrustumCulling,
    core_pipeline::tonemapping::Tonemapping,
    ecs::world::CommandQueue,
    input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll},
    mesh::PrimitiveTopology,
    prelude::*,
    tasks::AsyncComputeTaskPool,
    window::RequestRedraw,
    winit::{EventLoopProxyWrapper, WinitUserEvent::WakeUp},
};
use solitaire_solver::{Board, HashMap};

use crate::{
    CurrentBoard,
    solver::{BackgroundTask, FeasibleConstellations},
};

/// Highest peg count included in the graph.
///
/// Raising this is the intended way to scale the scene up, but the layer sizes grow
/// steeply (see the table in the module docs) and the whole graph is 1_679_072 nodes.
/// Past ~16 the per-node entity approach in [`spawn_graph`] is likely to need
/// replacing with a custom instanced renderer.
const MAX_PEGS: usize = 12;

/// Vertical distance between two layers.
///
/// Generous on purpose: the upper layers hold tens of thousands of boards and read as
/// a solid surface if the layers sit close enough together to occlude the moves
/// running between them.
const LAYER_HEIGHT: f32 = 2.0;

/// Centre-to-centre spacing used to size a layer's disc - see [`layer_radius`].
const NODE_SPACING: f32 = 0.06;

/// Kept well under [`NODE_SPACING`] so a dense layer still reads as separate boards.
const NODE_RADIUS: f32 = 0.015;

/// Keyboard fly speed, as a fraction of the orbit distance per second.
///
/// Relative to the distance rather than absolute so that a keypress covers the same
/// part of the screen whether you are looking at the whole funnel or at one board.
const FLY_SPEED: f32 = 0.8;

pub struct GraphPlugin;

impl Plugin for GraphPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_graph_camera);
        app.add_systems(
            Update,
            build_graph.run_if(resource_added::<FeasibleConstellations>),
        );
        app.add_systems(
            Update,
            spawn_graph.run_if(resource_added::<ConstellationGraph>),
        );
        app.add_systems(
            Update,
            (orbit_camera, fly_camera, highlight_current).run_if(resource_exists::<ShowGraph>),
        );
        app.add_systems(Update, toggle_on_key);
        app.add_observer(toggle_graph);
    }
}

/// Set while the graph scene is the visible one.
#[derive(Resource)]
pub struct ShowGraph;

#[derive(Default, Event)]
pub struct ToggleGraph;

/// Marks the perspective camera the graph is drawn with.
#[derive(Component)]
pub struct GraphCamera;

/// Marks the sphere that tracks the player's current board.
#[derive(Component)]
struct CurrentBoardMarker;

/// Orbit state for [`GraphCamera`], in spherical coordinates about [`Self::focus`].
#[derive(Component)]
struct Orbit {
    focus: Vec3,
    radius: f32,
    yaw: f32,
    pitch: f32,
}

impl Default for Orbit {
    fn default() -> Self {
        Self {
            focus: Vec3::ZERO,
            radius: 30.0,
            yaw: 0.6,
            pitch: 0.35,
        }
    }
}

impl Orbit {
    /// Frames the whole funnel.
    ///
    /// Derived from the graph's own extent rather than tuned by hand, so changing
    /// [`MAX_PEGS`] or [`LAYER_HEIGHT`] still opens with all of it on screen.
    fn frame(graph: &ConstellationGraph) -> Self {
        let height = (MAX_PEGS - 1) as f32 * LAYER_HEIGHT;
        let width = 2.0 * layer_radius(graph.layer(MAX_PEGS).len());
        Self {
            focus: Vec3::new(0.0, height / 2.0, 0.0),
            // bevy's default vertical fov is 45 degrees, so fitting an extent takes
            // about 1.2x it in distance - the rest is breathing room.
            radius: height.max(width) * 1.6,
            ..default()
        }
    }

    fn transform(&self) -> Transform {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        let offset = Vec3::new(cp * sy, sp, cp * cy) * self.radius;
        Transform::from_translation(self.focus + offset).looking_at(self.focus, Vec3::Y)
    }
}

/// The derived graph. Nodes are ordered by ascending peg count, so each layer is a
/// contiguous range - see [`ConstellationGraph::layer`].
#[derive(Resource)]
pub struct ConstellationGraph {
    /// world position per node
    pub nodes: Vec<Vec3>,
    /// normalized board -> index into [`Self::nodes`]
    pub index: HashMap<Board, u32>,
    /// `(from, to)` with `from` having exactly one peg more than `to`
    pub edges: Vec<(u32, u32)>,
    /// start offset of each peg count into [`Self::nodes`], length `MAX_PEGS + 2`
    layer_starts: Vec<u32>,
}

impl ConstellationGraph {
    /// Index range of all nodes with `pegs` pegs.
    fn layer(&self, pegs: usize) -> std::ops::Range<usize> {
        self.layer_starts[pegs] as usize..self.layer_starts[pegs + 1] as usize
    }
}

fn spawn_graph_camera(mut commands: Commands) {
    let orbit = Orbit::default();
    commands.spawn((
        Camera3d::default(),
        Camera {
            // starts hidden; `toggle_graph` flips this against the 2d camera
            is_active: false,
            ..default()
        },
        // The default tonemapper is TonyMcMapface, which needs a LUT that only ships
        // with the "tonemapping_luts" feature. That feature is deliberately off to
        // keep the wasm bundle small, so pick one that needs no LUT - otherwise the
        // whole scene renders black.
        Tonemapping::ReinhardLuminance,
        DistanceFog {
            color: Color::srgb_u8(43, 44, 47),
            falloff: FogFalloff::Linear {
                start: 20.,
                end: 60.,
            },
            ..default()
        },
        orbit.transform(),
        orbit,
        GraphCamera,
    ));
}

/// Derives the graph from the feasible set on the async pool.
///
/// Follows the same task shape as the stages in `solver.rs`: hand back a
/// [`CommandQueue`], let `solver::poll_task` apply it, and wake the winit event loop
/// because the app runs reactively and would otherwise not draw the result.
fn build_graph(
    mut commands: Commands,
    feasible: Res<FeasibleConstellations>,
    wake: Res<EventLoopProxyWrapper>,
) {
    info!("building constellation graph (<= {MAX_PEGS} pegs) ...");
    let thread_pool = AsyncComputeTaskPool::get();
    let entity = commands.spawn_empty().id();
    let feasible = feasible.0.clone();
    let wake = wake.clone();
    let task = thread_pool.spawn(async move {
        let graph = derive_graph(&feasible);
        info!(
            "constellation graph: {} nodes, {} edges",
            graph.nodes.len(),
            graph.edges.len()
        );

        let mut command_queue = CommandQueue::default();
        command_queue.push(move |world: &mut World| {
            world.insert_resource(graph);
            world.entity_mut(entity).remove::<BackgroundTask>();
        });
        wake.send_event(WakeUp).unwrap();
        command_queue
    });
    commands.entity(entity).insert(BackgroundTask { task });
}

fn derive_graph(feasible: &solitaire_solver::HashSet<Board>) -> ConstellationGraph {
    // bucket by peg count. `count_pegs` is the popcount, i.e. exactly the layer index.
    let mut layers: Vec<Vec<Board>> = vec![Vec::new(); MAX_PEGS + 1];
    for board in feasible.iter().copied() {
        let pegs = board.count_pegs();
        if pegs <= MAX_PEGS {
            layers[pegs].push(board);
        }
    }

    // Sort by the compressed 33-bit key so node order - and therefore the layout - is
    // identical across runs regardless of the hash set's iteration order.
    for layer in &mut layers {
        layer.sort_unstable_by_key(|b| b.to_compressed_repr());
    }

    let mut nodes = Vec::with_capacity(layers.iter().map(Vec::len).sum());
    let mut index = HashMap::default();
    let mut layer_starts = Vec::with_capacity(MAX_PEGS + 2);
    for layer in &layers {
        layer_starts.push(nodes.len() as u32);
        for board in layer {
            index.insert(*board, nodes.len() as u32);
            nodes.push(Vec3::ZERO);
        }
    }
    layer_starts.push(nodes.len() as u32);

    // Edges. A move always removes exactly one peg, so an edge out of a node in layer
    // k always lands in layer k-1; if the target is feasible it is therefore already
    // in `index`, and a hit there is the whole membership test.
    let mut edges = Vec::new();
    // skip(2): the 1-peg board is solved and has no moves left, and layer 0 is empty
    for layer in layers.iter().skip(2) {
        for board in layer {
            let from = index[board];
            for mov in board.get_legal_moves() {
                let successor = board.mov(mov).normalize();
                if let Some(&to) = index.get(&successor) {
                    edges.push((from, to));
                }
            }
        }
    }
    // Sort before dedup: distinct moves can normalize to the same successor (boards
    // with a nontrivial stabilizer), and those duplicates are not adjacent in move
    // order, so a bare dedup would leave them in.
    edges.sort_unstable();
    edges.dedup();

    let mut graph = ConstellationGraph {
        nodes,
        index,
        edges,
        layer_starts,
    };
    layout(&mut graph);
    graph
}

/// Radius of the disc a layer of `count` nodes is spread over.
///
/// Area grows with the node count, so node density - and therefore how dense the
/// picture looks - stays roughly constant from layer to layer. The floor keeps the
/// handful of layers near the apex from degenerating into a point; without it the
/// bottom third of the funnel is too small to see the individual boards in.
fn layer_radius(count: usize) -> f32 {
    const MIN_RADIUS: f32 = 0.35;
    (NODE_SPACING * (count as f32 / std::f32::consts::PI).sqrt()).max(MIN_RADIUS)
}

/// Places nodes: height from the peg count, and the horizontal position from a
/// top-down barycentric pass.
///
/// The widest layer is seeded with a sunflower disc, then each layer below is placed
/// at the centroid of its predecessors in the layer above. Going *downwards* is what
/// makes this work - a bottom-up pass from the single solved board would put every
/// centroid at the origin and collapse the whole graph to a line.
fn layout(graph: &mut ConstellationGraph) {
    let widest = graph.layer(MAX_PEGS);
    let count = widest.len();
    let radius = layer_radius(count);
    // Vogel's model: golden-angle increments with sqrt-spaced radii give an even disc.
    let golden_angle = std::f32::consts::PI * (3.0 - 5.0f32.sqrt());
    for (rank, node) in widest.clone().enumerate() {
        let r = radius * ((rank as f32 + 0.5) / count as f32).sqrt();
        let theta = golden_angle * rank as f32;
        graph.nodes[node] = Vec3::new(r * theta.cos(), 0.0, r * theta.sin());
    }

    // Predecessor sums per node, filled layer by layer as we walk down.
    for pegs in (1..MAX_PEGS).rev() {
        let mut sum = vec![Vec3::ZERO; graph.layer(pegs).len()];
        let mut n = vec![0u32; sum.len()];
        let base = graph.layer(pegs).start;
        // edges are sorted by `from`, so the ones out of layer pegs+1 are contiguous,
        // but a plain scan is cheap enough and keeps this readable.
        for &(from, to) in &graph.edges {
            let to = to as usize;
            if graph.layer(pegs).contains(&to) {
                sum[to - base] += graph.nodes[from as usize];
                n[to - base] += 1;
            }
        }
        for (i, node) in graph.layer(pegs).enumerate() {
            // Every feasible board below the widest layer has at least one feasible
            // predecessor one layer up, so `n` is only ever 0 if MAX_PEGS cut it off.
            if n[i] > 0 {
                graph.nodes[node] = sum[i] / n[i] as f32;
            }
        }
        spread_layer(graph, pegs);
    }

    for pegs in 1..=MAX_PEGS {
        let y = (pegs - 1) as f32 * LAYER_HEIGHT;
        for node in graph.layer(pegs) {
            graph.nodes[node].y = y;
        }
    }
}

/// Fraction of a layer allowed inside [`layer_radius`] when scaling it.
///
/// The remaining tail is clamped to the rim. Set from the shape of the data: the
/// barycentric radii are heavily skewed, so scaling on the largest radius would size
/// the disc for a handful of far-out boards and squash the rest into the middle.
const SPREAD_PERCENTILE: f32 = 0.98;

/// Re-centres one layer on the axis and scales it out to fill [`layer_radius`].
///
/// Averaging ~10 predecessors pulls every node towards the layer centroid by roughly
/// `1/sqrt(10)`, which compounds: without this each layer is about three times
/// narrower than the one above it and everything below the top few layers collapses
/// into a spike along the axis.
///
/// Scaling is uniform, so the clustering the barycentric pass found survives: boards
/// that share predecessors stay bunched, and a layer shows its real density - a dense
/// core with a thinner rim - rather than being flattened into an even disc. The
/// scale comes from a high percentile rather than the maximum, with the tail past it
/// clamped to the rim, which is what stops the far-out boards from being flung
/// outside the scene entirely.
fn spread_layer(graph: &mut ConstellationGraph, pegs: usize) {
    let layer = graph.layer(pegs);
    let count = layer.len();
    if count < 2 {
        // the apex, and the single 2-peg board
        for node in layer {
            graph.nodes[node] = Vec3::ZERO;
        }
        return;
    }

    let centroid = layer.clone().map(|i| graph.nodes[i]).sum::<Vec3>() / count as f32;
    let mut radii: Vec<f32> = layer
        .clone()
        .map(|i| (graph.nodes[i] - centroid).length())
        .collect();
    radii.sort_unstable_by(f32::total_cmp);

    let pivot = radii[((count as f32 * SPREAD_PERCENTILE) as usize).min(count - 1)];
    if pivot <= f32::EPSILON {
        return;
    }
    let radius = layer_radius(count);
    let scale = radius / pivot;

    for node in layer {
        let offset = (graph.nodes[node] - centroid) * scale;
        graph.nodes[node] = offset.clamp_length_max(radius).with_y(0.0);
    }
}

/// Spawns the scene once the graph is ready.
///
/// Nodes share one mesh and one material per layer so bevy can batch them, and all
/// the edges of a layer pair go into a single line-list mesh - one draw call per pair
/// instead of an entity per edge.
fn spawn_graph(
    mut commands: Commands,
    graph: Res<ConstellationGraph>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    camera: Single<(&mut Orbit, &mut Transform), With<GraphCamera>>,
    mut request_redraw: MessageWriter<RequestRedraw>,
) {
    let (mut orbit, mut camera_transform) = camera.into_inner();
    *orbit = Orbit::frame(&graph);
    *camera_transform = orbit.transform();

    let sphere = meshes.add(Sphere::new(NODE_RADIUS).mesh().ico(2).unwrap());

    for pegs in 1..=MAX_PEGS {
        let material = materials.add(StandardMaterial {
            base_color: layer_color(pegs),
            perceptual_roughness: 0.6,
            ..default()
        });
        let batch: Vec<_> = graph
            .layer(pegs)
            .map(|i| {
                (
                    Mesh3d(sphere.clone()),
                    MeshMaterial3d(material.clone()),
                    Transform::from_translation(graph.nodes[i]),
                )
            })
            .collect();
        commands.spawn_batch(batch);
    }

    // one line-list mesh per layer pair
    for pegs in 2..=MAX_PEGS {
        let layer = graph.layer(pegs);
        let mut positions = Vec::new();
        for &(from, to) in &graph.edges {
            if layer.contains(&(from as usize)) {
                positions.push(graph.nodes[from as usize].to_array());
                positions.push(graph.nodes[to as usize].to_array());
            }
        }
        if positions.is_empty() {
            continue;
        }
        let normals = vec![[0.0f32, 1.0, 0.0]; positions.len()];
        let mut mesh = Mesh::new(
            PrimitiveTopology::LineList,
            RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
        );
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
        let material = materials.add(StandardMaterial {
            base_color: layer_color(pegs).with_alpha(0.25),
            unlit: true,
            alpha_mode: AlphaMode::Blend,
            ..default()
        });
        commands.spawn((
            Mesh3d(meshes.add(mesh)),
            MeshMaterial3d(material),
            // the mesh spans a whole layer pair, so per-object culling buys nothing
            NoFrustumCulling,
        ));
    }

    // the sphere that tracks the player's current board
    commands.spawn((
        Mesh3d(meshes.add(Sphere::new(NODE_RADIUS * 6.0).mesh().ico(3).unwrap())),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::WHITE,
            emissive: LinearRgba::rgb(2.0, 2.0, 2.0),
            ..default()
        })),
        Visibility::Hidden,
        Transform::default(),
        CurrentBoardMarker,
    ));

    commands.spawn((
        DirectionalLight {
            illuminance: 6_000.0,
            // 129k instances make shadow casting the first thing to fall over
            shadows_enabled: false,
            ..default()
        },
        Transform::default().looking_to(Vec3::new(-0.4, -1.0, -0.6), Vec3::Y),
    ));
    commands.insert_resource(GlobalAmbientLight {
        color: Color::WHITE,
        brightness: 400.0,
        ..default()
    });

    request_redraw.write(RequestRedraw);
}

/// Blue at the apex through to red at the widest layer.
fn layer_color(pegs: usize) -> Color {
    let t = (pegs - 1) as f32 / (MAX_PEGS - 1) as f32;
    Color::hsl(240.0 * (1.0 - t), 0.75, 0.55)
}

/// Moves the marker sphere onto the node for the board the player is on.
fn highlight_current(
    board: Res<CurrentBoard>,
    graph: Option<Res<ConstellationGraph>>,
    marker: Single<(&mut Transform, &mut Visibility), With<CurrentBoardMarker>>,
) {
    let Some(graph) = graph else { return };
    let (mut transform, mut visibility) = marker.into_inner();
    match graph.index.get(&board.0.normalize()) {
        Some(&i) => {
            transform.translation = graph.nodes[i as usize];
            *visibility = Visibility::Visible;
        }
        // the played board is above MAX_PEGS for most of a game
        None => *visibility = Visibility::Hidden,
    }
}

/// Left-drag orbits, scroll zooms, right-drag pans.
///
/// The app is reactive (`WinitSettings::desktop_app`), so every change has to ask for
/// a redraw or the view freezes until some other input happens to wake it.
fn orbit_camera(
    mouse: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    scroll: Res<AccumulatedMouseScroll>,
    camera: Single<(&mut Orbit, &mut Transform), With<GraphCamera>>,
    mut request_redraw: MessageWriter<RequestRedraw>,
) {
    let (mut orbit, mut transform) = camera.into_inner();
    let mut changed = false;

    if mouse.pressed(MouseButton::Left) && motion.delta != Vec2::ZERO {
        orbit.yaw -= motion.delta.x * 0.005;
        orbit.pitch = (orbit.pitch + motion.delta.y * 0.005).clamp(
            -std::f32::consts::FRAC_PI_2 + 0.05,
            std::f32::consts::FRAC_PI_2 - 0.05,
        );
        changed = true;
    }

    if mouse.pressed(MouseButton::Right) && motion.delta != Vec2::ZERO {
        let right = *transform.right();
        let up = *transform.up();
        let scale = orbit.radius * 0.001;
        orbit.focus += (-right * motion.delta.x + up * motion.delta.y) * scale;
        changed = true;
    }

    if scroll.delta.y != 0.0 {
        orbit.radius = (orbit.radius * (1.0 - scroll.delta.y * 0.1)).clamp(1.0, 200.0);
        changed = true;
    }

    if changed {
        *transform = orbit.transform();
        request_redraw.write(RequestRedraw);
    }
}

/// `G` toggles the graph, next to `F` for fullscreen and `D` for the fps overlay.
fn toggle_on_key(input: Res<ButtonInput<KeyCode>>, mut commands: Commands) {
    if input.just_pressed(KeyCode::KeyG) {
        commands.trigger(ToggleGraph);
    }
}

/// Flies the camera with `WASD`, `space` up and `shift` down.
///
/// Moves the point the camera orbits, so the direction you are looking is preserved
/// and the mouse controls keep working unchanged around the new position. `W`/`S` run
/// along the ground rather than along the view direction, so looking down at the
/// funnel and pressing `W` moves over it instead of into it.
fn fly_camera(
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    camera: Single<(&mut Orbit, &mut Transform), With<GraphCamera>>,
    mut request_redraw: MessageWriter<RequestRedraw>,
) {
    let (mut orbit, mut transform) = camera.into_inner();

    let forward = transform.forward();
    let ground_forward = Vec3::new(forward.x, 0.0, forward.z).normalize_or_zero();
    let right = Vec3::new(-ground_forward.z, 0.0, ground_forward.x);

    let mut direction = Vec3::ZERO;
    if keys.pressed(KeyCode::KeyW) {
        direction += ground_forward;
    }
    if keys.pressed(KeyCode::KeyS) {
        direction -= ground_forward;
    }
    if keys.pressed(KeyCode::KeyD) {
        direction += right;
    }
    if keys.pressed(KeyCode::KeyA) {
        direction -= right;
    }
    if keys.pressed(KeyCode::Space) {
        direction += Vec3::Y;
    }
    if keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight) {
        direction -= Vec3::Y;
    }

    let Some(direction) = direction.try_normalize() else {
        return;
    };
    let step = orbit.radius * FLY_SPEED * time.delta_secs();
    orbit.focus += direction * step;
    *transform = orbit.transform();
    request_redraw.write(RequestRedraw);
}

/// Swaps which camera is active.
///
/// That is the whole switch: the 2d board is drawn by `ShapePainter` and `Text2d`,
/// which only render through the `Core2d` graph, and the graph's meshes only render
/// through `Core3d`. Deactivating one camera therefore hides everything belonging to
/// that scene without touching any of its entities.
fn toggle_graph(
    _: On<ToggleGraph>,
    mut commands: Commands,
    show_graph: Option<Res<ShowGraph>>,
    mut game_camera: Single<&mut Camera, (With<crate::GameCamera>, Without<GraphCamera>)>,
    mut graph_camera: Single<&mut Camera, With<GraphCamera>>,
    mut request_redraw: MessageWriter<RequestRedraw>,
) {
    let show = show_graph.is_none();
    if show {
        commands.insert_resource(ShowGraph);
    } else {
        commands.remove_resource::<ShowGraph>();
    }
    game_camera.is_active = !show;
    graph_camera.is_active = show;
    request_redraw.write(RequestRedraw);
}
