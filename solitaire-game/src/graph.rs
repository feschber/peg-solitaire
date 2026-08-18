//! A 3d view of the feasible constellation graph.
//!
//! Every node is one feasible constellation - a board that lies on at least one
//! complete solution - and every edge is a legal move. Height is the peg count, so
//! all edges point downwards. Feasible-board counts grow from the single solved board,
//! peak part-way up, then shrink back down as peg count approaches the (near-)unique
//! starting board - so the whole graph reads as an hourglass, not a pure funnel, widest
//! around its middle rather than at the top. See `ConstellationGraph::widest_pegs`.
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
    asset::{AssetPath, RenderAssetUsages, embedded_asset, embedded_path},
    core_pipeline::tonemapping::Tonemapping,
    ecs::world::CommandQueue,
    input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll},
    mesh::{Indices, PrimitiveTopology},
    prelude::*,
    render::render_resource::AsBindGroup,
    shader::ShaderRef,
    tasks::AsyncComputeTaskPool,
    ui::IsDefaultUiCamera,
    window::{CursorGrabMode, CursorOptions, PrimaryWindow, RequestRedraw},
    winit::{EventLoopProxy, EventLoopProxyWrapper, WinitUserEvent, WinitUserEvent::WakeUp},
};
use solitaire_solver::{Board, HashMap};

use crate::{
    CurrentBoard,
    solver::{BackgroundTask, FeasibleConstellations},
};

const EDGE_ALPHA: f32 = 0.02;

/// Highest peg count included in the graph.
///
/// Raising this is the intended way to scale the scene up, but the layer sizes grow
/// steeply (see the table in the module docs) and the whole graph is 1_679_072 nodes.
/// [`build_meshes`] already merges and spatially chunks every layer, so raising this
/// mainly costs the one-time mesh build (more vertex data to duplicate per instance,
/// more chunks - see its docs) rather than per-frame entity overhead. Past ~16 that
/// build itself may need a real instanced renderer to stay off the main thread.
const MAX_PEGS: usize = 32;

/// Vertical distance between two layers.
///
/// Generous on purpose: the upper layers hold tens of thousands of boards and read as
/// a solid surface if the layers sit close enough together to occlude the moves
/// running between them.
const LAYER_HEIGHT: f32 = 2.0;

/// Centre-to-centre spacing used to size a layer's disc - see [`layer_radius`].
const NODE_SPACING: f32 = 0.20;

/// Kept well under [`NODE_SPACING`] so a dense layer still reads as separate boards.
const NODE_RADIUS: f32 = 0.01;

/// Keyboard fly speed, as a fraction of the orbit distance per second.
///
/// Relative to the distance rather than absolute so that a keypress covers the same
/// part of the screen whether you are looking at the whole funnel or at one board.
const FLY_SPEED: f32 = 0.8;

pub struct GraphPlugin;

impl Plugin for GraphPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "graph.wgsl");
        app.add_plugins(MaterialPlugin::<GraphMaterial>::default());
        app.init_resource::<CameraMode>();
        app.init_resource::<BuildSettings>();
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
            prune_unreachable_edges.run_if(resource_added::<ShowGraph>),
        );
        app.add_systems(
            Update,
            (
                (orbit_camera, orbit_pan_keys).run_if(resource_equals(CameraMode::Orbit)),
                fly_camera.run_if(resource_equals(CameraMode::Fly)),
                highlight_current,
                toggle_camera_mode,
                rebuild_on_key,
            )
                .run_if(resource_exists::<ShowGraph>),
        );
        app.add_systems(Update, toggle_on_key);
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

/// Marks an edge-layer-chunk mesh entity, so [`prune_unreachable_edges`] can find and
/// replace them each time the graph is shown, without touching node entities (which
/// stay as-is - only edges get pruned).
#[derive(Component)]
struct EdgeMesh;

/// Marks every chunk mesh entity, node and edge alike, so [`switch_layout`] can clear
/// the whole scene in one query. [`EdgeMesh`] entities carry both.
#[derive(Component)]
struct GraphChunk;

/// Which of the two node layouts the graph is drawn with - switched with `L`.
///
/// Both are kept deliberately, because they answer different questions and neither
/// replaces the other: [`layout`] is the graph-drawing one (layers stacked by peg count,
/// relaxed to shorten edges, so the *move structure* is what you see), while
/// [`layout_cube`] plots each board's compressed representation straight into a cube, so
/// what you see is the shape of the *key space*. Before this was a choice, `derive_graph`
/// ran `layout` and then overwrote every position with `layout_cube` unconditionally - so
/// the relaxation work was computed and thrown away, and only the cube was ever visible.
#[derive(Resource, Default, Clone, Copy, PartialEq, Eq, Debug)]
enum GraphLayout {
    /// [`layout`] - an hourglass of barycentrically-relaxed layers, height from peg count.
    Hourglass,
    /// [`layout_cube`] - position straight from `Board::to_compressed_repr`.
    #[default]
    Cube,
}

impl GraphLayout {
    fn next(self) -> Self {
        match self {
            Self::Hourglass => Self::Cube,
            Self::Cube => Self::Hourglass,
        }
    }
}

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
    /// Frames the whole shape.
    ///
    /// Measured off the node positions themselves rather than reconstructed from
    /// [`LAYER_HEIGHT`] and [`layer_radius`], so it opens with all of the graph on screen
    /// under either [`GraphLayout`]. The reconstructed version only described [`layout`]'s
    /// geometry, and framed [`layout_cube`] - whose extent has nothing to do with those
    /// constants, and which is not centred on the origin - well off-screen.
    fn frame(graph: &ConstellationGraph) -> Self {
        let (min, max) = aabb_of(graph.nodes.iter().copied());
        let extent = max - min;
        Self {
            focus: (min + max) * 0.5,
            // bevy's default vertical fov is 45 degrees, so fitting an extent takes
            // about 1.2x it in distance - the rest is breathing room.
            radius: (extent.max_element() * 1.6).max(1.0),
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
    /// the peg count with the most nodes - see [`ConstellationGraph::find_widest_pegs`].
    /// *Not* generally [`MAX_PEGS`]: feasible-board counts grow from the single solved
    /// board, peak somewhere in the middle, then shrink back down towards the single
    /// (near-)starting board, so past that peak the widest layer is strictly below
    /// `MAX_PEGS`. Set once by [`layout`], read by both it and [`Orbit::frame`].
    widest_pegs: usize,
}

impl ConstellationGraph {
    /// Index range of all nodes with `pegs` pegs.
    fn layer(&self, pegs: usize) -> std::ops::Range<usize> {
        self.layer_starts[pegs] as usize..self.layer_starts[pegs + 1] as usize
    }

    /// The peg count with the most nodes - see [`Self::widest_pegs`]'s doc comment for
    /// why this isn't just [`MAX_PEGS`].
    fn find_widest_pegs(&self) -> usize {
        (1..=MAX_PEGS)
            .max_by_key(|&pegs| self.layer(pegs).len())
            .expect("MAX_PEGS >= 1")
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
    edges: Vec<EdgeChunk>,
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
        // Bevy defaults every camera to 4x MSAA, which is disproportionately expensive
        // for thin line primitives specifically: a solid triangle only needs extra
        // samples along its silhouette, but a 1px-wide line is silhouette everywhere it
        // touches, so ~every pixel it covers pays the 4x cost. Measured: turning MSAA
        // off roughly doubled fps at the worst (edge-dense, up-close) viewpoint - a
        // bigger win than reducing edge overdraw itself has managed so far.
        Msaa::Off,
        GraphCamera,
    )
}

#[derive(Asset, TypePath, AsBindGroup, Clone)]
struct GraphMaterial {
    #[uniform(0)]
    color: LinearRgba,
    alpha_mode: AlphaMode,
}

impl GraphMaterial {
    fn opaque(color: Color) -> Self {
        Self {
            color: color.to_linear().with_alpha(1.0),
            alpha_mode: AlphaMode::Opaque,
        }
    }
    fn additive(color: Color, intensity: f32) -> Self {
        let color = color.to_linear();
        Self {
            color: LinearRgba::new(
                color.red * intensity,
                color.green * intensity,
                color.blue * intensity,
                0.0,
            ),
            alpha_mode: AlphaMode::Add,
        }
    }
}

impl Material for GraphMaterial {
    fn vertex_shader() -> ShaderRef {
        shader()
    }
    fn fragment_shader() -> ShaderRef {
        shader()
    }
    fn alpha_mode(&self) -> AlphaMode {
        self.alpha_mode
    }
    fn enable_prepass() -> bool {
        false
    }
    fn enable_shadows() -> bool {
        false
    }
}

fn shader() -> ShaderRef {
    ShaderRef::Path(AssetPath::from_path_buf(embedded_path!("graph.wgsl")).with_source("embedded"))
}

fn node_mesh(radius: f32, subdivisions: u32) -> Mesh {
    let mut mesh = Sphere::new(radius)
        .mesh()
        .ico(subdivisions)
        .unwrap()
        .with_removed_attribute(Mesh::ATTRIBUTE_NORMAL)
        .with_removed_attribute(Mesh::ATTRIBUTE_UV_0);
    mesh.asset_usage = RenderAssetUsages::RENDER_WORLD;
    mesh
}

/// Spawns the two graph cameras as entirely separate entities - an orbit camera and a
/// free-flying one - rather than one entity switching control schemes. `Orbit`/
/// `FreeFly` being on only one entity each is what lets every other system in this
/// module keep querying "the orbit camera" / "the fly camera" just by requiring that
/// component, with no extra marker needed; [`toggle_camera_mode`] is the only place
/// that has to know about both at once, to swap which is [`Camera::is_active`].
fn spawn_graph_camera(mut commands: Commands) {
    let orbit = Orbit::default();
    let transform = orbit.transform();
    commands.spawn((graph_camera_bundle(), orbit.transform(), orbit));
    // seeded with an arbitrary transform - `toggle_camera_mode` overwrites it with the
    // orbit camera's current view the first time `O` is pressed, before it ever renders
    commands.spawn((graph_camera_bundle(), transform, FreeFly::default()));
}

/// Derives the graph and its render meshes from the feasible set on the async pool,
/// once the solver hands the feasible set over.
fn build_graph(
    mut commands: Commands,
    feasible: Res<FeasibleConstellations>,
    settings: Res<BuildSettings>,
    wake: Res<EventLoopProxyWrapper>,
) {
    spawn_build_task(&mut commands, &feasible.0, *settings, wake.clone());
}

/// Spawns the derive-plus-mesh-build task.
///
/// Follows the same task shape as the stages in `solver.rs`: hand back a
/// [`CommandQueue`], let `solver::poll_task` apply it, and wake the winit event loop so
/// a background result still gets drawn. Building the meshes here too, rather than in
/// [`spawn_graph`], keeps the per-vertex work (millions of floats once merged - see
/// [`build_meshes`]) off the main thread; the main thread only ever does the cheap
/// `Assets<Mesh>::add` + spawn.
///
/// Shared with [`switch_layout`], which rebuilds from scratch rather than re-laying-out
/// in place. Re-deriving the nodes and edges is a fraction of the total cost, and it
/// keeps every expensive step here on the background pool - an in-place version would
/// have had to either clone `index` (one entry per node) into the task or block the main
/// thread for the whole relaxation.
fn spawn_build_task(
    commands: &mut Commands,
    feasible: &solitaire_solver::HashSet<Board>,
    settings: BuildSettings,
    // the proxy itself rather than the resource wrapper, because that is what
    // `EventLoopProxyWrapper`'s `Deref` + `clone` at the call sites actually hands over
    wake: EventLoopProxy<WinitUserEvent>,
) {
    info!("building constellation graph (<= {MAX_PEGS} pegs, {settings:?}) ...");
    let thread_pool = AsyncComputeTaskPool::get();
    let entity = commands.spawn_empty().id();
    let feasible = feasible.clone();
    let task = thread_pool.spawn(async move {
        let graph = derive_graph(&feasible, settings.layout);
        info!(
            "constellation graph: {} nodes, {} edges",
            graph.nodes.len(),
            graph.edges.len()
        );
        let meshes = build_meshes(&graph, settings);
        info!(
            "graph meshes: {} node chunks, {} edge chunks",
            meshes.nodes.len(),
            meshes.edges.len()
        );

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

/// Clears the scene and rebuilds it: `L` switches [`GraphLayout`], `[`/`]` halve/double
/// the chunk size, `-`/`=` halve/double the edge budget.
///
/// Both are here so the two knobs this module has can be swept from one session without
/// rebuilding the binary or having to find the same viewpoint again - which is the only
/// way to compare them honestly, since the readout is an eyeballed FPS number.
fn rebuild_on_key(
    mut commands: Commands,
    input: Res<ButtonInput<KeyCode>>,
    feasible: Option<Res<FeasibleConstellations>>,
    graph: Option<Res<ConstellationGraph>>,
    mut settings: ResMut<BuildSettings>,
    chunks: Query<Entity, With<GraphChunk>>,
    wake: Res<EventLoopProxyWrapper>,
) {
    let switch = input.just_pressed(KeyCode::KeyL);
    let finer = input.just_pressed(KeyCode::BracketLeft);
    let coarser = input.just_pressed(KeyCode::BracketRight);
    let thinner = input.just_pressed(KeyCode::Minus);
    let denser = input.just_pressed(KeyCode::Equal);
    if !(switch || finer || coarser || thinner || denser) {
        return;
    }
    // `graph` being absent means either the first build or a previous rebuild is still in
    // flight, and starting a second one would race it into the same resources
    let (Some(feasible), Some(_)) = (feasible, graph) else {
        return;
    };

    if switch {
        settings.layout = settings.layout.next();
    }
    if finer {
        settings.chunk_size = (settings.chunk_size * 0.5).max(32.0);
    }
    if coarser {
        settings.chunk_size *= 2.0;
    }
    if thinner {
        settings.edge_budget = (settings.edge_budget / 2).max(1);
    }
    if denser {
        // no ceiling: past the busiest chunk's size this is simply "no decimation"
        settings.edge_budget *= 2;
    }

    for chunk in &chunks {
        commands.entity(chunk).despawn();
    }
    // removing this is what re-arms `spawn_graph`'s `resource_added` condition, so the
    // scene gets respawned - and reframed for the new layout's extent - when this lands
    commands.remove_resource::<ConstellationGraph>();
    spawn_build_task(&mut commands, &feasible.0, *settings, wake.clone());
}

fn derive_graph(
    feasible: &solitaire_solver::HashSet<Board>,
    graph_layout: GraphLayout,
) -> ConstellationGraph {
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
        widest_pegs: 0, // placeholder - the layout pass below sets the real value
    };
    match graph_layout {
        GraphLayout::Hourglass => layout(&mut graph),
        GraphLayout::Cube => {
            // `layout` is otherwise the one that fills this in, and it is part of the
            // graph rather than of either layout
            graph.widest_pegs = graph.find_widest_pegs();
            layout_cube(&mut graph);
        }
    }
    graph
}

/// Target primitive count per spatial chunk - see [`build_meshes`].
///
/// A grid's resolution is derived from this and the number of things going into it, so
/// thin layers (a handful of nodes) get a single chunk - same as the old one-mesh-
/// per-layer approach - while a dense layer gets many. Small enough that orbiting close
/// to one part of a dense layer only pulls a handful of chunks into the frustum, large
/// enough that the chunk count stays well below the per-node-entity counts this
/// replaced.
///
/// This is a real trade in *both* directions, which is why it is a [`ChunkSize`] resource
/// rather than only a constant: every chunk is a separate entity in a sorted render phase
/// (see [`build_edge_meshes`]), so finer chunking buys culling and pays draw calls. At the
/// full graph, 8.58M edges at this size come out as ~7.5k edge chunks, i.e. ~7.5k draw
/// calls a frame - and the viewpoint that actually hurts is the one where culling rejects
/// almost nothing, so there the draw calls are pure loss. Sweep it with `[` and `]`.
const DEFAULT_CHUNK_SIZE: f32 = 1024.0;

/// Default [`BuildSettings::edge_budget`] - the edge count above which a chunk gets
/// thinned. See [`decimation_level`].
///
/// Set well under [`DEFAULT_CHUNK_SIZE`] on purpose: the chunk grid is sized so a layer
/// *averages* that many edges per cell, so a budget equal to it would only touch the
/// above-average cells and leave the total roughly where it started. The edge pass needs
/// to lose most of its 8.58M primitives, not trim the tail.
const DEFAULT_EDGE_BUDGET: usize = 128;

/// Everything a graph (re)build is parameterized by - see [`rebuild_on_key`], which
/// sweeps both at runtime so they can be A/B'd from one session at one viewpoint rather
/// than across rebuilds.
#[derive(Resource, Clone, Copy, Debug)]
struct BuildSettings {
    layout: GraphLayout,
    /// target primitives per chunk - see [`DEFAULT_CHUNK_SIZE`]
    chunk_size: f32,
    /// max edges kept per chunk before [`decimation_level`] starts thinning it
    edge_budget: usize,
}

impl Default for BuildSettings {
    fn default() -> Self {
        Self {
            layout: GraphLayout::default(),
            chunk_size: DEFAULT_CHUNK_SIZE,
            edge_budget: DEFAULT_EDGE_BUDGET,
        }
    }
}

/// Bounding box of a set of positions. Empty input gives an inverted (non-finite) box,
/// which [`ChunkGrid::new`] treats as a single cell.
fn aabb_of(positions: impl IntoIterator<Item = Vec3>) -> (Vec3, Vec3) {
    positions.into_iter().fold(
        (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)),
        |(min, max), p| (min.min(p), max.max(p)),
    )
}

/// The box one chunk group actually occupies, cut into `divisions` cells per axis.
///
/// Derived from the positions themselves rather than from [`layer_radius`], which is
/// what makes it hold under either layout - see [`GraphLayout`]. The disc-shaped version
/// this replaced baked in [`layout`]'s geometry (each layer a flat disc on XZ, `y` fixed
/// by peg count) and *folded everything outside that disc into the rim cells*. Under
/// [`layout_cube`], where a layer's `y` varies as widely as its `x` and `z` and the
/// coordinates have nothing to do with `layer_radius`, that left most of a layer sharing
/// a handful of cells whose bounding boxes spanned the whole shape - so the frustum
/// culling this chunking exists to enable had very little left to reject.
#[derive(Clone, Copy, Default)]
struct ChunkGrid {
    min: Vec3,
    cell: Vec3,
    divisions: IVec3,
}

impl ChunkGrid {
    /// Cuts `min..max` into roughly cube-shaped cells, aiming for `target` positions in
    /// each - see [`ChunkSize`].
    ///
    /// Divisions are spread over the axes in proportion to the extent along each, so a
    /// layout that keeps a layer flat gets a 2d grid and one that doesn't gets a 3d one,
    /// with no special-casing either way: asking for `n_i` proportional to `extent_i`
    /// with the product equal to the wanted cell count gives `n_i = k * extent_i` for
    /// `k = (cells / product of extents)^(1/axes)`. Axes with no extent get one
    /// division and are left out of that product, which is what stops a flat layer from
    /// collapsing `k` to zero.
    fn new(min: Vec3, max: Vec3, count: usize, target: f32) -> Self {
        let extent = (max - min).max(Vec3::ZERO);
        let spread: Vec<usize> = (0..3).filter(|&i| extent[i] > f32::EPSILON).collect();
        let product: f32 = spread.iter().map(|&i| extent[i]).product();
        let cells = (count as f32 / target).ceil().max(1.0);
        let k = if spread.is_empty() || !product.is_normal() {
            0.0
        } else {
            (cells / product).powf(1.0 / spread.len() as f32)
        };

        let mut grid = Self {
            min,
            // 1.0 rather than 0.0 on the flat axes: `cell_of` divides by this, and a
            // zero-extent axis has to floor to cell 0 rather than produce a NaN
            cell: Vec3::ONE,
            divisions: IVec3::ONE,
        };
        for i in spread {
            grid.divisions[i] = ((k * extent[i]).round() as i32).max(1);
            grid.cell[i] = extent[i] / grid.divisions[i] as f32;
        }
        grid
    }

    /// Which cell `pos` falls in.
    ///
    /// The clamp is a boundary guard - a position exactly at `max`, or float error at a
    /// cell edge - not the wholesale fold-in the disc version did. Edge midpoints, which
    /// genuinely can sit outside a node layer's box, get their own grid instead; see
    /// [`build_edge_meshes`].
    fn cell_of(&self, pos: Vec3) -> IVec3 {
        let raw = (pos - self.min) / self.cell;
        IVec3::new(
            raw.x.floor() as i32,
            raw.y.floor() as i32,
            raw.z.floor() as i32,
        )
        .clamp(IVec3::ZERO, self.divisions - IVec3::ONE)
    }
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
///
/// This duplication is a real ceiling, not just a memory nice-to-have: raising
/// [`MAX_PEGS`] toward the full-size graph and bumping this sphere to `ico(2)`
/// (confirmed by hand) is enough to exhaust memory badly enough to crash the whole
/// desktop session, not just the app. The proper fix, when this needs revisiting, is
/// real GPU instancing - one shared base mesh plus a *static, write-once* per-instance
/// position buffer (node positions never change after layout, so unlike Bevy's
/// automatic GPU-preprocessing this needs no compute shader - it would work fine on
/// WebGL2). Bevy's own `examples/shader_advanced/custom_shader_instancing.rs` shows
/// the shape of it: a custom WGSL shader, `SpecializedMeshPipeline`, and a hand-managed
/// `RenderDevice` buffer. Note that example still needs `NoFrustumCulling` (per-instance
/// positions aren't reflected in the entity's bounding box), so it would have to keep
/// the spatial chunking here rather than replace it - instancing only fixes memory,
/// not culling.
///
/// Why not just spawn one entity per node and let Bevy's automatic instancing handle
/// it? On this crate's `webgl2` target there's no compute-shader support, so Bevy's
/// fast/GPU-driven batching path is unavailable; the CPU fallback
/// (`extract_meshes_for_cpu_building` in `bevy_pbr`) rebuilds *every* entity's
/// `MeshUniform` from scratch *every frame*, with no `Changed<Transform>` skip - unlike
/// the compute-shader path, which is change-detection-gated. Confirmed by reading the
/// source, not assumed. For ~129k+ static (never-moving) node entities that's a real,
/// unavoidable-in-stock-Bevy per-frame CPU tax, which is the actual reason nodes are
/// merged into a handful of chunk meshes here instead of left as individual entities.
///
/// **Chunking's actual limit** (measured, not theoretical): frustum culling can only
/// exclude geometry that's genuinely outside the camera's field of view. Orbiting near
/// the *rim* of a layer looking across it works great - most chunks are behind or to
/// the side, so culling drops them. But positioned near the *axis*, at the narrow neck
/// where one layer's edges converge into the next, looking outward, nearly the entire
/// layer-pair is legitimately in frame - there's nothing off to the side to cull.
/// Confirmed by counting: at that viewpoint 93-95% of all edges were still
/// `ViewVisibility` true regardless of chunk size (tested from the shipped
/// [`TARGET_CHUNK_NODES`] down to 32, i.e. ~19x more/smaller chunks - negligible
/// difference). No spatial partitioning scheme fixes that, because it isn't a
/// culling failure; it's fill-rate for genuinely-visible geometry. Confirmed
/// separately: framerate drops further at 4K vs windowed with everything else held
/// equal, which is the signature of a fill-rate (pixels shaded), not vertex-count or
/// CPU, bottleneck. A real fix from here would have to reduce pixels touched per
/// visible edge (distance-based fade/thinning, e.g.), not improve what's culled.
fn build_meshes(graph: &ConstellationGraph, settings: BuildSettings) -> GraphMeshes {
    let sphere = node_mesh(NODE_RADIUS, 0);
    let local_positions = sphere
        .attribute(Mesh::ATTRIBUTE_POSITION)
        .unwrap()
        .as_float3()
        .unwrap();
    let local_indices: Vec<u32> = sphere.indices().unwrap().iter().map(|i| i as u32).collect();

    // One grid per layer, sized from that layer's own positions, so each chunk mesh
    // gets a tight bounding box regardless of which layout produced them.
    let mut nodes = Vec::new();
    for pegs in 1..=MAX_PEGS {
        let layer = graph.layer(pegs);
        if layer.is_empty() {
            continue;
        }
        let (min, max) = aabb_of(layer.clone().map(|i| graph.nodes[i]));
        let grid = ChunkGrid::new(min, max, layer.len(), settings.chunk_size);

        let mut buckets: std::collections::HashMap<IVec3, Vec<usize>> =
            std::collections::HashMap::new();
        for node in layer {
            buckets
                .entry(grid.cell_of(graph.nodes[node]))
                .or_default()
                .push(node);
        }

        for bucket in buckets.into_values() {
            let mut positions = Vec::with_capacity(bucket.len() * local_positions.len());
            let mut indices = Vec::with_capacity(bucket.len() * local_indices.len());
            for node in bucket {
                let base = positions.len() as u32;
                let offset = graph.nodes[node];
                positions.extend(
                    local_positions
                        .iter()
                        .map(|&p| (Vec3::from(p) + offset).to_array()),
                );
                indices.extend(local_indices.iter().map(|i| i + base));
            }
            let mut mesh = Mesh::new(
                PrimitiveTopology::TriangleList,
                RenderAssetUsages::RENDER_WORLD,
            );
            mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
            mesh.insert_indices(Indices::U32(indices));
            nodes.push((pegs, mesh));
        }
    }

    let edges = build_edge_meshes(&graph.nodes, &graph.layer_starts, &graph.edges, settings);

    GraphMeshes { nodes, edges }
}

/// Merges a set of edges into per-chunk line-list meshes - shared by [`build_meshes`]
/// (the full edge set) and [`prune_unreachable_edges`] (whatever subset of it is still
/// reachable from the current board).
///
/// Chunked by each edge's own midpoint, in a grid built from the *midpoints'* bounding
/// box rather than either endpoint layer's: an edge's `to` node sits one layer down, and
/// neither layout keeps it directly "under" its predecessors, so a `from`-only chunk key
/// produces boxes that balloon to cover wherever this chunk's `to` ends happen to land.
/// Confirmed by measurement when this was `from`-keyed: 87% of edge chunks (95% of all
/// edges) were still "visible" from a single fixed viewpoint at the narrow neck just
/// below the widest layer - the chunking was barely culling anything there.
///
/// Two passes over each layer's edges rather than one: the grid needs the bounding box
/// before it can place anything, and at this scale keeping every midpoint around to
/// avoid recomputing it would cost more memory than the meshes themselves.
///
/// Note every chunk is a separate entity in [`Transparent3d`], which is a *sorted* phase
/// that only batches adjacent items - so each one costs its own draw call, sort key and
/// visibility check, and finer chunking is not free even where it culls well. Additive
/// blending is order-independent, so that sorting buys the edges nothing at all.
///
/// [`Transparent3d`]: bevy::core_pipeline::core_3d::Transparent3d
fn build_edge_meshes(
    nodes: &[Vec3],
    layer_starts: &[u32],
    edges: &[(u32, u32)],
    settings: BuildSettings,
) -> Vec<EdgeChunk> {
    let midpoint = |(from, to): (u32, u32)| (nodes[from as usize] + nodes[to as usize]) * 0.5;
    let mut chunks = Vec::new();
    let mut kept_total = 0usize;
    let mut busiest = 0usize;
    let mut by_level = [0usize; MAX_DECIMATION_LEVEL as usize + 1];

    for pegs in 1..=MAX_PEGS {
        let layer = layer_starts[pegs] as usize..layer_starts[pegs + 1] as usize;
        let slice = edges_from(edges, layer);
        if slice.is_empty() {
            continue;
        }

        let (min, max) = aabb_of(slice.iter().copied().map(midpoint));
        let grid = ChunkGrid::new(min, max, slice.len(), settings.chunk_size);

        let mut buckets: std::collections::HashMap<IVec3, Vec<(u32, u32)>> =
            std::collections::HashMap::new();
        for &edge in slice {
            buckets
                .entry(grid.cell_of(midpoint(edge)))
                .or_default()
                .push(edge);
        }

        for bucket in buckets.into_values() {
            busiest = busiest.max(bucket.len());
            let level = decimation_level(bucket.len(), settings.edge_budget);
            by_level[level as usize] += 1;

            let mut positions = Vec::with_capacity(bucket.len() * 2);
            for (from, to) in bucket {
                if !survives(from, to, level) {
                    continue;
                }
                positions.push(nodes[from as usize].to_array());
                positions.push(nodes[to as usize].to_array());
            }
            if positions.is_empty() {
                continue;
            }
            kept_total += positions.len() / 2;

            let mut mesh = Mesh::new(PrimitiveTopology::LineList, RenderAssetUsages::RENDER_WORLD);
            mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
            chunks.push(EdgeChunk { pegs, level, mesh });
        }
    }

    info!(
        "edges: {} -> {} kept ({:.1}%), busiest chunk {busiest}, chunks per decimation \
         level {by_level:?}",
        edges.len(),
        kept_total,
        100.0 * kept_total as f32 / edges.len().max(1) as f32,
    );
    chunks
}

/// One decimated edge chunk. `level` picks the material, whose brightness compensates for
/// how much of the chunk was thrown away - see [`edge_material`].
struct EdgeChunk {
    pegs: usize,
    level: u32,
    mesh: Mesh,
}

/// Cap on how far one chunk may be decimated, i.e. the brightest a surviving edge may be
/// made.
///
/// Compensation multiplies [`EDGE_ALPHA`] by `2^level`, so at 0.02 this bottoms out
/// around 32x (0.64) before a lone strand starts reading as a solid bright line instead
/// of one thread of a haze. Past it the very densest chunks stop being energy-preserving
/// and genuinely dim - which, for regions that were saturating anyway, is the right way
/// to run out of road.
const MAX_DECIMATION_LEVEL: u32 = 5;

/// How hard to thin a chunk holding `count` edges: keep `1 / 2^level` of them, chosen so
/// no chunk keeps more than about `budget`.
///
/// Density-adaptive rather than a flat fraction, which is the whole point. A uniform
/// keep-fraction is measurably effective (it is the only thing that moved the framerate)
/// but visibly wrong: thinning a dense tangle by 4x is statistically invisible because
/// what you see is the sum of thousands of overlapping strands, while thinning a sparse
/// region by 4x removes lines you were looking at individually. Bounding *per chunk*
/// leaves sparse chunks completely untouched - `count <= budget` gives level 0 - and
/// spends the decimation only where there is enough overlap to hide it.
fn decimation_level(count: usize, budget: usize) -> u32 {
    let budget = budget.max(1);
    if count <= budget {
        return 0;
    }
    // ceil(log2(count / budget)), via the bit length of the ratio
    let ratio = count.div_ceil(budget) as u64;
    let level = u64::BITS - (ratio - 1).leading_zeros();
    level.min(MAX_DECIMATION_LEVEL)
}

/// Whether an edge survives decimation at `level`, i.e. is in the `1 / 2^level` of edges
/// whose hash starts with `level` zero bits.
///
/// Hashed rather than "keep every n-th": the edge list is sorted by endpoint index, which
/// correlates strongly with position under either layout, so a stride would sample the
/// graph on a lattice and alias into visible structure. Being a pure function of the
/// endpoints also keeps the choice stable across rebuilds, so toggling some other setting
/// does not reshuffle which edges are drawn.
fn survives(from: u32, to: u32, level: u32) -> bool {
    level == 0 || hash32(from ^ hash32(to)) >> (u32::BITS - level) == 0
}

/// Chris Wellons' `lowbias32`.
fn hash32(mut h: u32) -> u32 {
    h ^= h >> 16;
    h = h.wrapping_mul(0x7feb_352d);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846c_a68b);
    h ^= h >> 16;
    h
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

/// Extra up/down relaxation passes after the initial seeding pass - see [`layout`].
///
/// Each pass is two full sweeps (up then down), so this is `2 * RELAXATION_PASSES`
/// barycenter recomputations of every layer past the first. Picked as "enough to
/// visibly tighten edges without a noticeable build-time cost" rather than derived -
/// layout runs once, off the main thread, so there's headroom to raise this if edges
/// still look slack.
const RELAXATION_PASSES: usize = 4;

/// Places nodes: height from the peg count, and the horizontal position from
/// iterated barycentric relaxation.
///
/// Feasible-board counts grow from the single solved board, peak somewhere in the
/// middle, then shrink back down as peg count approaches `MAX_PEGS` (there's only one
/// near-full starting board, same as there's only one solved one) - so the true
/// widest layer (`graph.widest_pegs`, found by [`ConstellationGraph::find_widest_pegs`])
/// is not generally the top layer. That layer is seeded with a sunflower disc and is
/// the one layer that never moves - every other layer's position is defined relative
/// to it, directly or transitively. Every other layer then gets an initial position
/// at the centroid of its predecessors (if below the widest layer) or successors (if
/// above), sweeping away from the anchor in each direction. Sweeping *away* from an
/// already-placed layer is what makes this work at all - sweeping into unplaced
/// territory would put every centroid at the origin and collapse that whole side to a
/// line.
///
/// A single sweep only ever pulls a layer towards the one neighboring layer it looked
/// at, though - it never revisits a layer once its other neighbor exists, so edges to
/// that other neighbor stay however long the initial pass happened to leave them.
/// Fixed by further relaxation rounds over every non-anchor layer, for
/// [`RELAXATION_PASSES`] rounds - but *not* by further one-directional sweeps: running
/// a successor-sweep and a predecessor-sweep back to back doesn't average the two,
/// it just lets whichever runs second completely overwrite the first's result (see
/// [`barycenter_from_neighbors`]'s doc comment), so alternating one-directional sweeps
/// converges to a fixed point defined by whichever direction ran last, not one that
/// accounts for both. [`barycenter_from_neighbors`] instead centres each layer on *all*
/// of its neighbors - predecessors and successors together - in one update, which is
/// the exact update that minimizes total squared edge length to every neighbor at
/// once; this is the barycenter method layered graph-drawing tools like Graphviz's
/// `dot` use for the equivalent step. [`spread_layer`] runs after every single-layer
/// update, for the same reason it already needed to in the initial pass: unopposed
/// averaging shrinks a layer towards a point, and the next layer processed needs this
/// one's *rescaled* position, not the raw centroid, when using it as a reference.
#[allow(unused)]
fn layout(graph: &mut ConstellationGraph) {
    graph.widest_pegs = graph.find_widest_pegs();
    let widest_pegs = graph.widest_pegs;

    sunflower_disc(graph, widest_pegs);

    // initial seed: sweep away from the anchor once in each direction
    for pegs in (1..widest_pegs).rev() {
        barycenter_from_predecessors(graph, pegs);
        spread_layer(graph, pegs);
    }
    for pegs in (widest_pegs + 1)..=MAX_PEGS {
        barycenter_from_successors(graph, pegs);
        spread_layer(graph, pegs);
    }

    for _ in 0..RELAXATION_PASSES {
        for pegs in 1..=MAX_PEGS {
            barycenter_from_neighbors(graph, pegs);
            spread_layer(graph, pegs);
        }
    }

    for pegs in 1..=MAX_PEGS {
        let y = (pegs - 1) as f32 * LAYER_HEIGHT;
        for node in graph.layer(pegs) {
            graph.nodes[node].y = y;
        }
    }
}

/// Evenly distributes one layer's nodes over its disc via Vogel's model
/// (golden-angle increments with sqrt-spaced radii give an even disc) - used both to
/// seed the anchor layer in [`layout`], and as [`spread_layer`]'s fallback when the
/// barycentric pass leaves a layer's nodes too tightly clustered to have a meaningful
/// direction to rescale outward from.
fn sunflower_disc(graph: &mut ConstellationGraph, pegs: usize) {
    let layer = graph.layer(pegs);
    let count = layer.len();
    let radius = layer_radius(count);
    let golden_angle = std::f32::consts::PI * (3.0 - 5.0f32.sqrt());
    for (rank, node) in layer.enumerate() {
        let r = radius * ((rank as f32 + 0.5) / count as f32).sqrt();
        let theta = golden_angle * rank as f32;
        graph.nodes[node] = Vec3::new(r * theta.cos(), 0.0, r * theta.sin());
    }
}

/// The slice of `edges` whose `from` endpoint lies in `range` (a node-index range,
/// e.g. from [`ConstellationGraph::layer`]) - relies on `derive_graph` having sorted
/// `edges` (primarily by `from`), turning what used to be a full linear scan per layer
/// into a binary search plus a scan of only that layer's own edges. Takes a raw slice
/// rather than `&ConstellationGraph` so [`prune_unreachable_edges`] can reuse it on a
/// pruned edge list that isn't part of a full graph.
fn edges_from(edges: &[(u32, u32)], range: std::ops::Range<usize>) -> &[(u32, u32)] {
    let start = edges.partition_point(|&(from, _)| (from as usize) < range.start);
    let end = edges.partition_point(|&(from, _)| (from as usize) < range.end);
    &edges[start..end]
}

/// Every node reachable by repeated moves (i.e. forward through the graph, always to
/// fewer pegs) starting from `start`, which has `start_pegs` pegs - see
/// [`prune_unreachable_edges`].
///
/// A move only ever removes a peg, so this is the *entire* set of boards the player
/// could still end up at from here - anything not in it can never be reached again
/// regardless of what's played next, no matter that it may have been reachable from
/// the very first board.
///
/// One layer at a time, outward from `start`: at each step the frontier is entirely
/// within one layer, so [`edges_from`] slices to just that layer's edges, and every
/// edge whose `from` is actually in the (possibly much smaller) frontier - not just
/// somewhere in that layer - both marks its `to` reachable and carries it into the
/// next layer's frontier.
fn reachable_from(
    layer_starts: &[u32],
    edges: &[(u32, u32)],
    start: u32,
    start_pegs: usize,
) -> std::collections::HashSet<u32> {
    let mut reachable = std::collections::HashSet::new();
    reachable.insert(start);
    let mut frontier: std::collections::HashSet<u32> = [start].into_iter().collect();
    for pegs in (1..start_pegs).rev() {
        let layer = layer_starts[pegs + 1] as usize..layer_starts[pegs + 2] as usize;
        let mut next_frontier = std::collections::HashSet::new();
        for &(from, to) in edges_from(edges, layer) {
            if frontier.contains(&from) && reachable.insert(to) {
                next_frontier.insert(to);
            }
        }
        frontier = next_frontier;
        if frontier.is_empty() {
            break;
        }
    }
    reachable
}

/// Repositions every node in layer `pegs` to the centroid of its predecessors (layer
/// `pegs + 1`, the only layer with edges into this one) - the down-sweep step of
/// [`layout`]'s relaxation.
fn barycenter_from_predecessors(graph: &mut ConstellationGraph, pegs: usize) {
    let base = graph.layer(pegs).start;
    let mut sum = vec![Vec3::ZERO; graph.layer(pegs).len()];
    let mut n = vec![0u32; sum.len()];
    for &(from, to) in edges_from(&graph.edges, graph.layer(pegs + 1)) {
        let i = to as usize - base;
        sum[i] += graph.nodes[from as usize];
        n[i] += 1;
    }
    for (i, node) in graph.layer(pegs).enumerate() {
        // Every feasible board below MAX_PEGS has at least one feasible predecessor
        // one layer up (it was reached by some move on some solution path), so `n` is
        // only ever 0 for boards at MAX_PEGS itself - callers never pass that in.
        if n[i] > 0 {
            graph.nodes[node] = sum[i] / n[i] as f32;
        }
    }
}

/// Repositions every node in layer `pegs` to the centroid of its successors (layer
/// `pegs - 1`, the only layer this one has edges into) - the up-sweep step of
/// [`layout`]'s relaxation.
fn barycenter_from_successors(graph: &mut ConstellationGraph, pegs: usize) {
    let base = graph.layer(pegs).start;
    let mut sum = vec![Vec3::ZERO; graph.layer(pegs).len()];
    let mut n = vec![0u32; sum.len()];
    for &(from, to) in edges_from(&graph.edges, graph.layer(pegs)) {
        let i = from as usize - base;
        sum[i] += graph.nodes[to as usize];
        n[i] += 1;
    }
    for (i, node) in graph.layer(pegs).enumerate() {
        // every non-apex board has at least one legal move, i.e. one successor
        if n[i] > 0 {
            graph.nodes[node] = sum[i] / n[i] as f32;
        }
    }
}

/// Repositions every node in layer `pegs` to the centroid of *all* its neighbors -
/// predecessors (layer `pegs + 1`) and successors (layer `pegs - 1`) combined into one
/// average - [`layout`]'s relaxation step.
///
/// Not the same as running [`barycenter_from_predecessors`] then
/// [`barycenter_from_successors`] (or the reverse) back to back: whichever ran second
/// would completely overwrite the first's result for every layer it touched, since
/// neither looks at the other's contribution - so alternating one-directional sweeps
/// converges to a fixed point defined by whichever direction's sweep runs last, not one
/// that jointly accounts for both neighbors. Averaging both in a single pass avoids
/// that: each update is the exact centroid of everything this layer connects to,
/// full stop, so there's no direction whose pull the next step silently discards.
fn barycenter_from_neighbors(graph: &mut ConstellationGraph, pegs: usize) {
    let base = graph.layer(pegs).start;
    let mut sum = vec![Vec3::ZERO; graph.layer(pegs).len()];
    let mut n = vec![0u32; sum.len()];
    if pegs < MAX_PEGS {
        for &(from, to) in edges_from(&graph.edges, graph.layer(pegs + 1)) {
            let i = to as usize - base;
            sum[i] += graph.nodes[from as usize];
            n[i] += 1;
        }
    }
    for &(from, to) in edges_from(&graph.edges, graph.layer(pegs)) {
        let i = from as usize - base;
        sum[i] += graph.nodes[to as usize];
        n[i] += 1;
    }
    for (i, node) in graph.layer(pegs).enumerate() {
        if n[i] > 0 {
            graph.nodes[node] = sum[i] / n[i] as f32;
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
///
/// Falls back to [`sunflower_disc`] instead of scaling when the barycentric pass left
/// a layer that isn't genuinely spread across *both* dimensions of its plane (see
/// [`spans_two_dimensions`]) - this is rare in the funnel's lower half, but routine
/// near `MAX_PEGS`: those layers are small and highly convergent (few boards, each
/// with many moves landing back among the same handful of successors), so a node's
/// position - the centroid of however many of those few successors it connects to -
/// is mathematically confined to their convex hull. With only one or two distinct
/// successor positions to average over, that hull is a point or a line segment, and
/// uniformly scaling a point or a line just produces a bigger point or line - it takes
/// an actual even distribution to turn that into a disc.
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
    if !spans_two_dimensions(graph, layer.clone(), centroid) {
        sunflower_disc(graph, pegs);
        return;
    }

    let mut radii: Vec<f32> = layer
        .clone()
        .map(|i| (graph.nodes[i] - centroid).length())
        .collect();
    radii.sort_unstable_by(f32::total_cmp);

    let pivot = radii[((count as f32 * SPREAD_PERCENTILE) as usize).min(count - 1)];
    if pivot <= f32::EPSILON {
        // shouldn't happen once `spans_two_dimensions` passed, but dividing by it next
        // would be a NaN if it somehow did
        sunflower_disc(graph, pegs);
        return;
    }
    let radius = layer_radius(count);
    let scale = radius / pivot;

    for node in layer {
        let offset = (graph.nodes[node] - centroid) * scale;
        graph.nodes[node] = offset.clamp_length_max(radius).with_y(0.0);
    }
}

/// Whether a layer's raw (pre-rescale) point cloud actually spans both dimensions of
/// its plane, rather than having collapsed onto a point or a line - see
/// [`spread_layer`]. A nonzero spread radius alone doesn't rule out a collinear cloud
/// (any two distinct points already give one a meaningful "radius"), so this instead
/// looks at the eigenvalues of the cloud's XZ covariance matrix about `centroid`: a
/// genuinely 2d cloud has two eigenvalues of comparable magnitude, while one that's
/// collapsed towards a point or a line has one (or both) collapse towards zero.
fn spans_two_dimensions(
    graph: &ConstellationGraph,
    layer: std::ops::Range<usize>,
    centroid: Vec3,
) -> bool {
    let count = layer.len() as f32;
    let (mut var_x, mut var_z, mut cov_xz) = (0.0f32, 0.0f32, 0.0f32);
    for i in layer {
        let d = graph.nodes[i] - centroid;
        var_x += d.x * d.x;
        var_z += d.z * d.z;
        cov_xz += d.x * d.z;
    }
    var_x /= count;
    var_z /= count;
    cov_xz /= count;

    let trace = var_x + var_z;
    if trace <= f32::EPSILON {
        return false; // collapsed onto (essentially) a single point
    }
    let det = var_x * var_z - cov_xz * cov_xz;
    let min_eigenvalue = (trace - (trace * trace - 4.0 * det).max(0.0).sqrt()) / 2.0;
    // the narrow axis needs to carry a non-negligible fraction of the total spread,
    // or the cloud reads as a line no matter how much the wide axis carries
    min_eigenvalue / trace > 1e-4
}

/// Plots each board's compressed representation straight into a cube - the
/// [`GraphLayout::Cube`] layout.
///
/// Nothing here looks at the edges: this shows the shape of the *key space*
/// (`Board::to_compressed_repr` read as three base-`WIDTH` digits) rather than the move
/// structure [`layout`] draws, which is why both are kept.
///
/// Iterates `index` rather than the feasible set it used to, which is the same node set
/// by construction - every node is in `index` - so the "no idx for board" warning that
/// used to guard the lookup was unreachable and is gone.
fn layout_cube(graph: &mut ConstellationGraph) {
    // const WIDTH: u64 = 52015;
    const WIDTH: u64 = 2048;
    // const WIDTH: u64 = 92682;
    const WIDTH_SQ: u64 = WIDTH * WIDTH;
    const SCALE: f64 = 50.0;

    // split borrow: writing `nodes` while reading `index`, both fields of `graph`
    let (nodes, index) = (&mut graph.nodes, &graph.index);
    for (board, &idx) in index {
        let compr = board.to_compressed_repr();
        // let compr = board.0;
        // const POW_2_47: u64 = 1 << 47;
        // let compr: u64 = rand::random_range(0..POW_2_47);

        let layer = compr / WIDTH_SQ;
        let row = (compr % WIDTH_SQ) / WIDTH;
        let col = compr % WIDTH;

        // let layer = 0;
        // let row = compr / WIDTH;
        // let col = compr % WIDTH;

        nodes[idx as usize] = Vec3::new(
            (col as f64 / SCALE) as f32,
            (layer as f64 / SCALE) as f32,
            (row as f64 / SCALE) as f32,
        );
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
    mut materials: ResMut<Assets<GraphMaterial>>,
    camera: Single<(&mut Orbit, &mut Transform), With<GraphCamera>>,
    mut request_redraw: MessageWriter<RequestRedraw>,
) {
    let (mut orbit, mut camera_transform) = camera.into_inner();
    *orbit = Orbit::frame(&graph);
    *camera_transform = orbit.transform();

    // one material per peg count, shared across that layer's chunks - many chunks
    // would otherwise each add an identical material asset
    let mut node_materials: HashMap<usize, Handle<GraphMaterial>> = HashMap::default();
    let mut edge_materials: std::collections::HashMap<(usize, u32), Handle<GraphMaterial>> =
        std::collections::HashMap::new();

    // `mem::take` rather than borrowing: these meshes are merged megabytes-large
    // buffers, and moving them into `Assets<Mesh>` avoids cloning that data around
    for (pegs, mesh) in std::mem::take(&mut graph_meshes.nodes) {
        let material = node_materials
            .entry(pegs)
            .or_insert_with(|| materials.add(GraphMaterial::opaque(layer_color(pegs))))
            .clone();
        commands.spawn((Mesh3d(meshes.add(mesh)), MeshMaterial3d(material), GraphChunk));
    }

    for EdgeChunk { pegs, level, mesh } in std::mem::take(&mut graph_meshes.edges) {
        let material = edge_materials
            .entry((pegs, level))
            .or_insert_with(|| materials.add(edge_material(pegs, level)))
            .clone();
        commands.spawn((
            Mesh3d(meshes.add(mesh)),
            MeshMaterial3d(material),
            EdgeMesh,
            GraphChunk,
        ));
    }

    commands.remove_resource::<GraphMeshes>();

    // the sphere that tracks the player's current board - kept lit so `emissive` still
    // reads as a glow rather than a flat disc now that the funnel itself is unlit
    commands.spawn((
        Mesh3d(meshes.add(node_mesh(NODE_RADIUS * 6.0, 3))),
        MeshMaterial3d(materials.add(GraphMaterial::opaque(Color::WHITE))),
        Visibility::Hidden,
        Transform::default(),
        CurrentBoardMarker,
    ));

    request_redraw.write(RequestRedraw);
}

/// Rebuilds the edge meshes to only whatever's still [`reachable_from`] the current
/// board, every time the graph is (re)shown.
///
/// Lazy rather than eager: recomputed once when the graph opens (using whatever the
/// board is at that exact moment), not on every move regardless of whether the graph
/// is even being looked at - opening the graph after playing deep into a game should
/// still only draw the (now much smaller) set of moves still reachable from here, but
/// there's no reason to pay for that rebuild on moves where the graph never gets shown.
///
/// Follows the same background-task/`CommandQueue` shape as [`build_graph`] (and needs
/// to: pruning re-buckets and rebuilds every affected chunk's line-list mesh, the same
/// per-vertex work that justified moving that off the main thread in the first place) -
/// except this one only ever replaces [`EdgeMesh`]-marked entities, leaving nodes and
/// everything else untouched, per the design call to keep every board visible and
/// prune only the connections between them.
fn prune_unreachable_edges(
    mut commands: Commands,
    graph: Option<Res<ConstellationGraph>>,
    board: Res<CurrentBoard>,
    settings: Res<BuildSettings>,
    wake: Res<EventLoopProxyWrapper>,
) {
    let Some(graph) = graph else { return };
    let normalized = board.0.normalize();
    let start_pegs = normalized.count_pegs();
    // not a graph node - e.g. above MAX_PEGS early in the game - nothing to prune
    // from, so leave whatever edges are already there rather than guess
    let Some(&start) = graph.index.get(&normalized) else {
        info!("DEBUG prune: board not in graph.index (pegs={start_pegs}), skipping");
        return;
    };

    let settings = *settings;
    let nodes = graph.nodes.clone();
    let edges = graph.edges.clone();
    let layer_starts = graph.layer_starts.clone();
    let total_edges = edges.len();

    let thread_pool = AsyncComputeTaskPool::get();
    let entity = commands.spawn_empty().id();
    let wake = wake.clone();
    let task = thread_pool.spawn(async move {
        let reachable = reachable_from(&layer_starts, &edges, start, start_pegs);
        let pruned: Vec<(u32, u32)> = edges
            .iter()
            .copied()
            .filter(|&(from, _)| reachable.contains(&from))
            .collect();
        info!(
            "DEBUG prune: start_pegs={start_pegs} reachable_nodes={} edges {total_edges} -> {}",
            reachable.len(),
            pruned.len()
        );

        let edge_meshes = build_edge_meshes(&nodes, &layer_starts, &pruned, settings);

        let mut command_queue = CommandQueue::default();
        command_queue.push(move |world: &mut World| {
            let old: Vec<Entity> = world
                .query_filtered::<Entity, With<EdgeMesh>>()
                .iter(world)
                .collect();
            for old_entity in old {
                world.despawn(old_entity);
            }

            let mut edge_materials: std::collections::HashMap<
                (usize, u32),
                Handle<GraphMaterial>,
            > = std::collections::HashMap::new();
            for EdgeChunk { pegs, level, mesh } in edge_meshes {
                let mesh_handle = world.resource_mut::<Assets<Mesh>>().add(mesh);
                let material = edge_materials
                    .entry((pegs, level))
                    .or_insert_with(|| {
                        world
                            .resource_mut::<Assets<GraphMaterial>>()
                            .add(edge_material(pegs, level))
                    })
                    .clone();
                world.spawn((
                    Mesh3d(mesh_handle),
                    MeshMaterial3d(material),
                    EdgeMesh,
                    GraphChunk,
                ));
            }

            world.entity_mut(entity).remove::<BackgroundTask>();
        });
        wake.send_event(WakeUp).unwrap();
        command_queue
    });
    commands.entity(entity).insert(BackgroundTask { task });
}

/// Blue at the apex through to red at the widest layer.
fn layer_color(pegs: usize) -> Color {
    let t = (pegs - 1) as f32 / (MAX_PEGS - 1) as f32;
    Color::hsl(360.0 * (1.0 - t), 0.75, 0.55)
}

/// Brightness scaled by `2^level` to compensate for the `1 / 2^level` of the chunk that
/// [`decimation_level`] threw away. Additive blending is linear, so this leaves the
/// expected accumulated brightness where it was and only adds grain - which is what makes
/// decimation a knob rather than a compromise.
fn edge_material(pegs: usize, level: u32) -> GraphMaterial {
    GraphMaterial::additive(layer_color(pegs), EDGE_ALPHA * (1u32 << level) as f32)
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
/// The [`RequestRedraw`] here (and in the other camera systems) is left over from when
/// `WinitSettings::desktop_app` made the app reactive; it is commented out in
/// `window.rs`, so the loop is continuous and these are redundant rather than load-
/// bearing. Kept because reactive mode is worth having back for a scene this expensive -
/// but note that while measuring, continuous is what you want.
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
/// Also moves `IsDefaultUiCamera` onto the newly active camera - see [`GameCamera`]'s
/// doc comment for why UI silently stops rendering without this - and grabs/releases
/// the OS cursor - fly mode wants raw, unbounded mouse motion for its look, which needs
/// the cursor confined to (and hidden over) the window; see [`set_cursor_grab`].
/// `toggle_graph` releases it too, so leaving the graph entirely while flying doesn't
/// strand the player's cursor grabbed over the 2d board.
fn toggle_camera_mode(
    mut commands: Commands,
    input: Res<ButtonInput<KeyCode>>,
    mut mode: ResMut<CameraMode>,
    mut orbit_active: Single<(Entity, &mut Camera), OrbitCameraFilter>,
    mut fly_active: Single<(Entity, &mut Camera), FlyCameraFilter>,
    cursor: Single<&mut CursorOptions, With<PrimaryWindow>>,
    mut request_redraw: MessageWriter<RequestRedraw>,
) {
    if !input.just_pressed(KeyCode::KeyO) {
        return;
    }
    let (orbit_entity, orbit_camera) = &mut *orbit_active;
    let (fly_entity, fly_camera) = &mut *fly_active;

    *mode = match *mode {
        CameraMode::Orbit => {
            orbit_camera.is_active = false;
            fly_camera.is_active = true;
            commands.entity(*orbit_entity).remove::<IsDefaultUiCamera>();
            commands.entity(*fly_entity).insert(IsDefaultUiCamera);
            set_cursor_grab(cursor.into_inner(), true);
            CameraMode::Fly
        }
        CameraMode::Fly => {
            fly_camera.is_active = false;
            orbit_camera.is_active = true;
            commands.entity(*fly_entity).remove::<IsDefaultUiCamera>();
            commands.entity(*orbit_entity).insert(IsDefaultUiCamera);
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

/// Filter for the 2d board's camera - see [`OrbitCameraFilter`].
type GameCameraFilter = (With<crate::GameCamera>, Without<GraphCamera>);

/// Swaps which camera is active.
///
/// That is the whole switch: the 2d board is drawn by `ShapePainter` and `Text2d`,
/// which only render through the `Core2d` graph, and the graph's meshes only render
/// through `Core3d`. Deactivating a camera therefore hides everything belonging to
/// its scene without touching any of its entities. Exactly one of the three cameras
/// (2d board, orbit, fly) ends up active: the 2d board when hidden, otherwise
/// whichever graph camera matches the current [`CameraMode`]. `IsDefaultUiCamera`
/// (see [`GameCamera`]'s doc comment) always follows the same one, or UI drawn without
/// an explicit target camera - like the fps overlay - silently renders through
/// whichever camera won Bevy's static "highest order, tie-broken by entity id"
/// fallback, which has nothing to do with which camera is actually active.
#[allow(clippy::too_many_arguments)]
fn toggle_graph(
    _: On<ToggleGraph>,
    mut commands: Commands,
    show_graph: Option<Res<ShowGraph>>,
    mode: Res<CameraMode>,
    game_camera: Single<(Entity, &mut Camera), GameCameraFilter>,
    orbit_cam: Single<(Entity, &mut Camera), OrbitCameraFilter>,
    fly_cam: Single<(Entity, &mut Camera), FlyCameraFilter>,
    cursor: Single<&mut CursorOptions, With<PrimaryWindow>>,
    mut request_redraw: MessageWriter<RequestRedraw>,
) {
    let show = show_graph.is_none();
    if show {
        commands.insert_resource(ShowGraph);
    } else {
        commands.remove_resource::<ShowGraph>();
    }
    let (game_entity, mut game_camera) = game_camera.into_inner();
    let (orbit_entity, mut orbit_camera) = orbit_cam.into_inner();
    let (fly_entity, mut fly_camera) = fly_cam.into_inner();

    game_camera.is_active = !show;
    let fly_mode = show && *mode == CameraMode::Fly;
    orbit_camera.is_active = show && !fly_mode;
    fly_camera.is_active = fly_mode;

    let active_entity = if !show {
        game_entity
    } else if fly_mode {
        fly_entity
    } else {
        orbit_entity
    };
    for entity in [game_entity, orbit_entity, fly_entity] {
        if entity == active_entity {
            commands.entity(entity).insert(IsDefaultUiCamera);
        } else {
            commands.entity(entity).remove::<IsDefaultUiCamera>();
        }
    }

    // hiding the graph always releases the cursor (the 2d board needs it free), even
    // if fly mode's grab is still logically active - showing it again re-grabs
    set_cursor_grab(cursor.into_inner(), fly_mode);

    request_redraw.write(RequestRedraw);
}
