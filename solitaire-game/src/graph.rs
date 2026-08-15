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
//! Toggled with the graph button or `G`. Starts in orbit mode: left-drag orbits,
//! right-drag pans, the wheel zooms, and `WASD` + `space`/`shift` pans the pivot.
//! `O` switches to a free-flying first-person mode instead, grabbing the mouse so it
//! always looks around (no drag needed) - `WASD` moves along the view direction (not
//! locked to the ground), `space`/`shift` still move straight up/down, and the wheel
//! adjusts fly speed there.

use bevy::{
    asset::RenderAssetUsages,
    core_pipeline::tonemapping::Tonemapping,
    ecs::world::CommandQueue,
    input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll},
    mesh::{Indices, PrimitiveTopology},
    prelude::*,
    tasks::AsyncComputeTaskPool,
    window::{CursorGrabMode, CursorOptions, PrimaryWindow, RequestRedraw},
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
/// [`build_meshes`] already merges and spatially chunks every layer, so raising this
/// mainly costs the one-time mesh build (more vertex data to duplicate per instance,
/// more chunks - see its docs) rather than per-frame entity overhead. Past ~16 that
/// build itself may need a real instanced renderer to stay off the main thread.
const MAX_PEGS: usize = 21;

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
const FLY_SPEED: f32 = 0.2;

pub struct GraphPlugin;

impl Plugin for GraphPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CameraMode>();
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
            (
                (orbit_camera, orbit_pan_keys).run_if(resource_equals(CameraMode::Orbit)),
                fly_camera.run_if(resource_equals(CameraMode::Fly)),
                highlight_current,
            )
                .run_if(resource_exists::<ShowGraph>),
        );
        app.add_systems(Update, (toggle_on_key, toggle_camera_mode));
        app.add_observer(toggle_graph);
    }
}

/// Which control scheme [`GraphCamera`] currently responds to - toggled by `O`.
#[derive(Resource, Default, Clone, Copy, PartialEq, Eq)]
enum CameraMode {
    #[default]
    Orbit,
    Fly,
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

    /// Forward direction implied by `yaw`/`pitch` alone - the camera-to-focus
    /// direction, i.e. `-offset` normalized. [`FreeFly`] uses the same formula for its
    /// own look direction, via the same yaw/pitch convention.
    fn forward(yaw: f32, pitch: f32) -> Vec3 {
        let (sy, cy) = yaw.sin_cos();
        let (sp, cp) = pitch.sin_cos();
        Vec3::new(-cp * sy, -sp, -cp * cy)
    }
}

/// State for the free-flying first-person camera - see [`CameraMode::Fly`].
///
/// Position lives directly on the entity's `Transform`; this only tracks orientation
/// and speed. Yaw/pitch use the same convention as [`Orbit`] (see [`Orbit::forward`]) -
/// not because the two modes hand off state (they don't, see [`toggle_camera_mode`]),
/// just so the same formula works for both.
#[derive(Component)]
struct FreeFly {
    yaw: f32,
    pitch: f32,
    /// world units/second, scroll-adjustable like [`Orbit::radius`] is
    speed: f32,
}

impl Default for FreeFly {
    fn default() -> Self {
        Self {
            yaw: 0.0,
            pitch: 0.0,
            speed: 5.0,
        }
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

/// Render meshes for [`ConstellationGraph`], built alongside it on the background
/// thread - see [`build_meshes`]. `spawn_graph` only has to register these as assets
/// and spawn one entity each.
///
/// Each layer (for nodes) or layer pair (for edges) is split into several spatial
/// chunks rather than one mesh apiece - see [`build_meshes`] for why. `usize` is the
/// peg count each mesh belongs to, used to pick its material.
#[derive(Resource)]
struct GraphMeshes {
    nodes: Vec<(usize, Mesh)>,
    edges: Vec<(usize, Mesh)>,
}

/// Bundle shared by both graph cameras - only the mode-specific state (an [`Orbit`] or
/// a [`FreeFly`]) and initial [`Transform`] differ between them.
fn graph_camera_bundle() -> impl Bundle {
    (
        Camera3d::default(),
        Camera {
            // starts hidden; `toggle_graph` flips this against the 2d camera and
            // between the two graph cameras
            is_active: false,
            ..default()
        },
        // The default tonemapper is TonyMcMapface, which needs a LUT that only ships
        // with the "tonemapping_luts" feature. That feature is deliberately off to
        // keep the wasm bundle small, so pick one that needs no LUT - otherwise the
        // whole scene renders black.
        Tonemapping::ReinhardLuminance,
        // DistanceFog {
        //     color: Color::srgb_u8(43, 44, 47),
        //     falloff: FogFalloff::Linear {
        //         start: 20.,
        //         end: 60.,
        //     },
        //     ..default()
        // },
        GraphCamera,
    )
}

/// Spawns the two graph cameras as entirely separate entities - an orbit camera and a
/// free-flying one - rather than one entity switching control schemes. `Orbit`/
/// `FreeFly` being on only one entity each is what lets every other system in this
/// module keep querying "the orbit camera" / "the fly camera" just by requiring that
/// component, with no extra marker needed; [`toggle_camera_mode`] is the only place
/// that has to know about both at once, to swap which is [`Camera::is_active`].
fn spawn_graph_camera(mut commands: Commands) {
    let orbit = Orbit::default();
    commands.spawn((graph_camera_bundle(), orbit.transform(), orbit));
    // seeded with an arbitrary transform - `toggle_camera_mode` overwrites it with the
    // orbit camera's current view the first time `O` is pressed, before it ever renders
    commands.spawn((
        graph_camera_bundle(),
        Transform::default(),
        FreeFly::default(),
    ));
}

/// Derives the graph and its render meshes from the feasible set on the async pool.
///
/// Follows the same task shape as the stages in `solver.rs`: hand back a
/// [`CommandQueue`], let `solver::poll_task` apply it, and wake the winit event loop
/// because the app runs reactively and would otherwise not draw the result. Building
/// the meshes here too, rather than in [`spawn_graph`], keeps the per-vertex work
/// (millions of floats once merged - see [`build_meshes`]) off the main thread; the
/// main thread only ever does the cheap `Assets<Mesh>::add` + spawn.
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
        let meshes = build_meshes(&graph);

        let mut command_queue = CommandQueue::default();
        command_queue.push(move |world: &mut World| {
            world.insert_resource(graph);
            world.insert_resource(meshes);
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

/// Target node count per spatial chunk - see [`build_meshes`].
///
/// A layer's grid resolution is derived from this (`sqrt(layer size / this)`), so
/// thin layers (a handful of nodes) get a single chunk - same as the old one-mesh-
/// per-layer approach - while the widest layer gets on the order of `68_326 / 1024
/// ≈ 66` chunks. Small enough that orbiting close to one part of a dense layer only
/// pulls a handful of chunks into the frustum, large enough that the chunk count
/// stays well below the per-node-entity counts this replaced.
const TARGET_CHUNK_NODES: f32 = 1024.0;

/// Maps a world position to its grid cell within a disc of the given `radius`,
/// split into a `grid * grid` array of cells - see [`build_meshes`].
fn chunk_of(pos: Vec3, radius: f32, grid: usize) -> (i32, i32) {
    let cell = (2.0 * radius / grid as f32).max(f32::EPSILON);
    let to_cell = |v: f32| (((v + radius) / cell).floor() as i32).clamp(0, grid as i32 - 1);
    (to_cell(pos.x), to_cell(pos.z))
}

/// Merges nodes and edges into per-chunk meshes - see [`GraphMeshes`].
///
/// One entity per node used to cost a transform, a visibility check and an entry in
/// the render extraction every frame, times up to 129_207 nodes; merging into static
/// meshes turns all of that into a one-time upload. But merging a whole layer into a
/// *single* mesh (as the edges already did before this) throws away per-object
/// culling: with the mesh's bounding box spanning the entire layer, orbiting close to
/// one corner of the widest layer still submits and rasterizes the layer's entire
/// geometry every frame - lines with one endpoint right next to the camera project to
/// huge screen-space spans, and there are tens of thousands of them, which is what
/// drove close-up framerate into the single digits even after the per-node-entity fix.
/// Chunking spatially (via [`chunk_of`]) keeps each mesh's bounding box tight, so
/// Bevy's ordinary per-entity frustum culling (re-enabled here by *not* using
/// `NoFrustumCulling`) drops the chunks the camera isn't looking at.
///
/// The trade from merging at all still applies: vertex data is duplicated per
/// instance instead of shared, so the local sphere is kept at `ico(1)` rather than
/// the single-entity version's `ico(2)` - nodes are unlit and only [`NODE_RADIUS`]
/// across, so the extra roundness wasn't visible anyway.
fn build_meshes(graph: &ConstellationGraph) -> GraphMeshes {
    let sphere = Sphere::new(NODE_RADIUS).mesh().ico(1).unwrap();
    let local_positions = sphere
        .attribute(Mesh::ATTRIBUTE_POSITION)
        .unwrap()
        .as_float3()
        .unwrap();
    let local_normals = sphere
        .attribute(Mesh::ATTRIBUTE_NORMAL)
        .unwrap()
        .as_float3()
        .unwrap();
    let local_indices: Vec<u32> = sphere.indices().unwrap().iter().map(|i| i as u32).collect();

    // peg count and chunk coordinate per node, needed by both passes below - edges
    // are chunked by their `from` node's chunk, reusing the node grid for that layer
    let mut node_pegs = vec![0usize; graph.nodes.len()];
    let mut node_chunk = vec![(0i32, 0i32); graph.nodes.len()];
    for pegs in 1..=MAX_PEGS {
        let layer = graph.layer(pegs);
        let grid = ((layer.len() as f32 / TARGET_CHUNK_NODES).sqrt().ceil() as usize).max(1);
        let radius = layer_radius(layer.len());
        for node in layer {
            node_pegs[node] = pegs;
            node_chunk[node] = chunk_of(graph.nodes[node], radius, grid);
        }
    }

    let mut node_buckets: std::collections::HashMap<(usize, i32, i32), Vec<usize>> =
        std::collections::HashMap::new();
    for pegs in 1..=MAX_PEGS {
        for node in graph.layer(pegs) {
            node_buckets
                .entry((pegs, node_chunk[node].0, node_chunk[node].1))
                .or_default()
                .push(node);
        }
    }
    let nodes = node_buckets
        .into_iter()
        .map(|((pegs, _, _), bucket)| {
            let mut positions = Vec::with_capacity(bucket.len() * local_positions.len());
            let mut normals = Vec::with_capacity(bucket.len() * local_normals.len());
            let mut indices = Vec::with_capacity(bucket.len() * local_indices.len());
            for node in bucket {
                let base = positions.len() as u32;
                let offset = graph.nodes[node];
                positions.extend(
                    local_positions
                        .iter()
                        .map(|&p| (Vec3::from(p) + offset).to_array()),
                );
                normals.extend_from_slice(local_normals);
                indices.extend(local_indices.iter().map(|i| i + base));
            }
            let mut mesh = Mesh::new(
                PrimitiveTopology::TriangleList,
                RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
            );
            mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
            mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
            mesh.insert_indices(Indices::U32(indices));
            (pegs, mesh)
        })
        .collect();

    let mut edge_buckets: std::collections::HashMap<(usize, i32, i32), Vec<(u32, u32)>> =
        std::collections::HashMap::new();
    for &(from, to) in &graph.edges {
        let (cx, cz) = node_chunk[from as usize];
        edge_buckets
            .entry((node_pegs[from as usize], cx, cz))
            .or_default()
            .push((from, to));
    }
    let edges = edge_buckets
        .into_iter()
        .map(|((pegs, _, _), bucket)| {
            let mut positions = Vec::with_capacity(bucket.len() * 2);
            for (from, to) in bucket {
                positions.push(graph.nodes[from as usize].to_array());
                positions.push(graph.nodes[to as usize].to_array());
            }
            let normals = vec![[0.0f32, 1.0, 0.0]; positions.len()];
            let mut mesh = Mesh::new(
                PrimitiveTopology::LineList,
                RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
            );
            mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
            mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
            (pegs, mesh)
        })
        .collect();

    GraphMeshes { nodes, edges }
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

/// Spawns the scene once the graph and its meshes are ready.
///
/// The heavy lifting - building the per-chunk meshes - already happened on the
/// background thread (see [`build_meshes`]); this just registers them as assets and
/// spawns one entity per chunk. Deliberately no `NoFrustumCulling` here (unlike the
/// per-layer meshes this replaced): each chunk's bounding box is tight enough that
/// Bevy's ordinary per-entity culling is exactly what makes chunking pay off.
fn spawn_graph(
    mut commands: Commands,
    graph: Res<ConstellationGraph>,
    mut graph_meshes: ResMut<GraphMeshes>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    camera: Single<(&mut Orbit, &mut Transform), With<GraphCamera>>,
    mut request_redraw: MessageWriter<RequestRedraw>,
) {
    let (mut orbit, mut camera_transform) = camera.into_inner();
    *orbit = Orbit::frame(&graph);
    *camera_transform = orbit.transform();

    // one material per peg count, shared across that layer's chunks - many chunks
    // would otherwise each add an identical material asset
    let mut node_materials: HashMap<usize, Handle<StandardMaterial>> = HashMap::default();
    let mut edge_materials: HashMap<usize, Handle<StandardMaterial>> = HashMap::default();

    // `mem::take` rather than borrowing: these meshes are merged megabytes-large
    // buffers, and moving them into `Assets<Mesh>` avoids cloning that data around
    for (pegs, mesh) in std::mem::take(&mut graph_meshes.nodes) {
        let material = node_materials
            .entry(pegs)
            .or_insert_with(|| {
                materials.add(StandardMaterial {
                    base_color: layer_color(pegs),
                    unlit: true,
                    ..default()
                })
            })
            .clone();
        commands.spawn((Mesh3d(meshes.add(mesh)), MeshMaterial3d(material)));
    }

    for (pegs, mesh) in std::mem::take(&mut graph_meshes.edges) {
        let material = edge_materials
            .entry(pegs)
            .or_insert_with(|| {
                materials.add(StandardMaterial {
                    base_color: layer_color(pegs).with_alpha(0.25),
                    unlit: true,
                    // alpha_mode: AlphaMode::Blend,
                    ..default()
                })
            })
            .clone();
        commands.spawn((Mesh3d(meshes.add(mesh)), MeshMaterial3d(material)));
    }

    commands.remove_resource::<GraphMeshes>();

    // the sphere that tracks the player's current board - kept lit so `emissive` still
    // reads as a glow rather than a flat disc now that the funnel itself is unlit
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

/// Pans the orbit pivot with `WASD`, `space` up and `shift` down - only in
/// [`CameraMode::Orbit`]; see [`fly_camera`] for the free-flying equivalent.
///
/// Moves the point the camera orbits, so the direction you are looking is preserved
/// and the mouse controls keep working unchanged around the new position. `W`/`S` run
/// along the ground rather than along the view direction, so looking down at the
/// funnel and pressing `W` moves over it instead of into it.
fn orbit_pan_keys(
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

/// Free-flying first-person camera, active only in [`CameraMode::Fly`].
///
/// The OS cursor is grabbed and hidden for the whole time this mode is active (see
/// [`toggle_camera_mode`]/[`toggle_graph`]), so the mouse always drives look here -
/// unlike [`orbit_camera`], no drag/hold is needed. `WASD` moves straight along the
/// current view direction rather than the ground plane - unlike [`orbit_pan_keys`],
/// this is meant to fly *through* the funnel, not glide over it. `space`/`shift` still
/// move along world up/down regardless of pitch, and the wheel adjusts fly speed
/// instead of a zoom radius, since free flight has no pivot to zoom toward.
fn fly_camera(
    motion: Res<AccumulatedMouseMotion>,
    scroll: Res<AccumulatedMouseScroll>,
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    camera: Single<(&mut FreeFly, &mut Transform), With<GraphCamera>>,
    mut request_redraw: MessageWriter<RequestRedraw>,
) {
    let (mut fly, mut transform) = camera.into_inner();
    let mut changed = false;

    if motion.delta != Vec2::ZERO {
        fly.yaw -= motion.delta.x * 0.005;
        fly.pitch = (fly.pitch + motion.delta.y * 0.005).clamp(
            -std::f32::consts::FRAC_PI_2 + 0.05,
            std::f32::consts::FRAC_PI_2 - 0.05,
        );
        changed = true;
    }

    if scroll.delta.y != 0.0 {
        fly.speed = (fly.speed * (1.0 - scroll.delta.y * 0.1)).clamp(0.1, 500.0);
    }

    if changed {
        transform.look_to(Orbit::forward(fly.yaw, fly.pitch), Vec3::Y);
    }

    let mut direction = Vec3::ZERO;
    if keys.pressed(KeyCode::KeyW) {
        direction += *transform.forward();
    }
    if keys.pressed(KeyCode::KeyS) {
        direction -= *transform.forward();
    }
    if keys.pressed(KeyCode::KeyD) {
        direction += *transform.right();
    }
    if keys.pressed(KeyCode::KeyA) {
        direction -= *transform.right();
    }
    if keys.pressed(KeyCode::Space) {
        direction += Vec3::Y;
    }
    if keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight) {
        direction -= Vec3::Y;
    }

    if let Some(direction) = direction.try_normalize() {
        transform.translation += direction * fly.speed * time.delta_secs();
        changed = true;
    }

    if changed {
        request_redraw.write(RequestRedraw);
    }
}

/// `O` swaps [`CameraMode`] between the orbit and free-flying cameras - two entirely
/// separate entities (see [`spawn_graph_camera`]) with their own [`Transform`] and
/// controls. Switching is nothing but flipping which one is [`Camera::is_active`] -
/// no state is copied between them, so each keeps whatever position/orientation it
/// was last left at and picks up exactly there next time it's switched back to.
///
/// Also grabs/releases the OS cursor - fly mode wants raw, unbounded mouse motion for
/// its look, which needs the cursor confined to (and hidden over) the window; see
/// [`set_cursor_grab`]. `toggle_graph` releases it too, so leaving the graph entirely
/// while flying doesn't strand the player's cursor grabbed over the 2d board.
fn toggle_camera_mode(
    input: Res<ButtonInput<KeyCode>>,
    mut mode: ResMut<CameraMode>,
    mut orbit_active: Single<&mut Camera, (With<Orbit>, Without<FreeFly>)>,
    mut fly_active: Single<&mut Camera, (With<FreeFly>, Without<Orbit>)>,
    cursor: Single<&mut CursorOptions, With<PrimaryWindow>>,
    mut request_redraw: MessageWriter<RequestRedraw>,
) {
    if !input.just_pressed(KeyCode::KeyO) {
        return;
    }

    *mode = match *mode {
        CameraMode::Orbit => {
            orbit_active.is_active = false;
            fly_active.is_active = true;
            set_cursor_grab(cursor.into_inner(), true);
            CameraMode::Fly
        }
        CameraMode::Fly => {
            fly_active.is_active = false;
            orbit_active.is_active = true;
            set_cursor_grab(cursor.into_inner(), false);
            CameraMode::Orbit
        }
    };
    request_redraw.write(RequestRedraw);
}

/// Locks and hides the cursor for [`CameraMode::Fly`]'s raw mouse-look, or restores
/// the normal free cursor - see [`toggle_camera_mode`]/[`toggle_graph`].
fn set_cursor_grab(mut cursor: Mut<CursorOptions>, grabbed: bool) {
    cursor.grab_mode = if grabbed {
        CursorGrabMode::Locked
    } else {
        CursorGrabMode::None
    };
    cursor.visible = !grabbed;
}

/// Filter for the orbit camera entity - needs `With<GraphCamera>` to disjoint-prove
/// against `game_camera`'s query below, and `With<Orbit>` + `Without<FreeFly>` to
/// disjoint-prove against [`FlyCameraFilter`]'s query - see [`toggle_graph`].
type OrbitCameraFilter = (With<GraphCamera>, With<Orbit>, Without<FreeFly>);

/// The fly camera's equivalent of [`OrbitCameraFilter`].
type FlyCameraFilter = (With<GraphCamera>, With<FreeFly>, Without<Orbit>);

/// Swaps which camera is active.
///
/// That is the whole switch: the 2d board is drawn by `ShapePainter` and `Text2d`,
/// which only render through the `Core2d` graph, and the graph's meshes only render
/// through `Core3d`. Deactivating a camera therefore hides everything belonging to
/// its scene without touching any of its entities. Exactly one of the three cameras
/// (2d board, orbit, fly) ends up active: the 2d board when hidden, otherwise
/// whichever graph camera matches the current [`CameraMode`].
#[allow(clippy::too_many_arguments)]
fn toggle_graph(
    _: On<ToggleGraph>,
    mut commands: Commands,
    show_graph: Option<Res<ShowGraph>>,
    mode: Res<CameraMode>,
    mut game_camera: Single<&mut Camera, (With<crate::GameCamera>, Without<GraphCamera>)>,
    orbit_cam: Single<&mut Camera, OrbitCameraFilter>,
    fly_cam: Single<&mut Camera, FlyCameraFilter>,
    cursor: Single<&mut CursorOptions, With<PrimaryWindow>>,
    mut request_redraw: MessageWriter<RequestRedraw>,
) {
    let show = show_graph.is_none();
    if show {
        commands.insert_resource(ShowGraph);
    } else {
        commands.remove_resource::<ShowGraph>();
    }
    game_camera.is_active = !show;
    let fly_mode = show && *mode == CameraMode::Fly;
    orbit_cam.into_inner().is_active = show && !fly_mode;
    fly_cam.into_inner().is_active = fly_mode;

    // hiding the graph always releases the cursor (the 2d board needs it free), even
    // if fly mode's grab is still logically active - showing it again re-grabs
    set_cursor_grab(cursor.into_inner(), fly_mode);

    request_redraw.write(RequestRedraw);
}
