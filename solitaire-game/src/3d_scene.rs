fn camera_setup_3d(mut commands: Commands /*  asset_server: &AssetServer */) {
    commands.spawn((
        Camera3d::default(),
        Camera {
            hdr: true,
            ..default()
        },
        camera_transform_3d(),
        DistanceFog {
            color: Color::srgb_u8(43, 44, 47),
            falloff: FogFalloff::Linear {
                start: 10.,
                end: 50.,
            },
            ..default()
        },
        // EnvironmentMapLight {
        //     diffuse_map: asset_server.load("environment_maps/pisa_diffuse_rgb9e5_zstd.ktx2"),
        //     specular_map: asset_server.load("environment_maps/pisa_specular_rgb9e5_zstd.ktx2"),
        //     intensity: 2000.0,
        //     ..default()
        // },
    ));
}

fn setup_3d_meshes(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.spawn((
        PointLight {
            intensity: 15_000_000.0,
            shadows_enabled: true,
            ..default()
        },
        Transform::from_xyz(-5.0, 9.0, 8.),
    ));
    // commands.spawn((
    //     DirectionalLight {
    //         illuminance: light_consts::lux::OVERCAST_DAY,
    //         shadows_enabled: true,
    //         ..default()
    //     },
    //     Transform {
    //         translation: Vec3::new(0.0, 0.0, 0.0),
    //         rotation: Quat::from_rotation_z(-2.5 * PI / 4.),
    //         ..default()
    //     },
    // ));
    let ground_plane = Plane3d::new(Vec3::Z, Vec2::splat(4.));
    commands.spawn((
        Mesh3d(meshes.add(ground_plane.mesh())),
        MeshMaterial3d(materials.add(Color::srgb(0.8, 0.8, 0.8))),
        Transform::from_xyz(0.0, 0.0, BOARD_POS),
    ));
}

fn camera_transform_3d() -> Transform {
    Transform::from_xyz(6., 3., 10.).looking_at(Vec3::new(0.0, 0.0, 0.0), Vec3::Z)
}
