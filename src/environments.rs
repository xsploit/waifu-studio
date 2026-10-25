//! Procedural low-poly stage environments drawn behind the avatar.
//! Everything is built from a handful of shared unit meshes and flat
//! materials (no images, no downloads). 1 unit = 1 m, the avatar stands at the
//! origin on y = 0 and the studio camera looks down -Z, so scenery lives at
//! negative Z and the space around the avatar stays clear.
use bevy::{light::NotShadowCaster, prelude::*};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvironmentKind {
    #[default]
    Disabled,
    CozyRoom,
    MoonlitGarden,
    NeonRooftop,
    RoseBedroom,
    VioletStudio,
    CyanLounge,
}

impl EnvironmentKind {
    pub const ALL: [Self; 7] = [
        Self::Disabled,
        Self::CozyRoom,
        Self::MoonlitGarden,
        Self::NeonRooftop,
        Self::RoseBedroom,
        Self::VioletStudio,
        Self::CyanLounge,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Disabled => "None",
            Self::CozyRoom => "Cozy room",
            Self::MoonlitGarden => "Moonlit garden",
            Self::NeonRooftop => "Neon rooftop",
            Self::RoseBedroom => "Rose bedroom",
            Self::VioletStudio => "Violet gaming studio",
            Self::CyanLounge => "Cyan gaming lounge",
        }
    }
}

#[derive(Resource, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EnvironmentSettings {
    pub kind: EnvironmentKind,
    /// Temporarily unloads the scene without changing its saved selection.
    #[serde(skip)]
    pub suppressed: bool,
}

impl EnvironmentSettings {
    pub fn is_active(&self) -> bool {
        self.kind != EnvironmentKind::Disabled && !self.suppressed
    }
}

/// Root of the spawned scene; despawning it releases every mesh and material.
#[derive(Component)]
pub struct EnvironmentRoot {
    pub kind: EnvironmentKind,
}

#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct EnvironmentSystems;

pub struct EnvironmentPlugin;

impl Plugin for EnvironmentPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<EnvironmentSettings>()
            .init_resource::<EnvironmentState>()
            .add_systems(Update, sync_environment.in_set(EnvironmentSystems));
    }
}

#[derive(Resource, Default)]
struct EnvironmentState {
    root: Option<Entity>,
    kind: EnvironmentKind,
}

/// Spawns only when the selected kind differs from the live scene; no asset
/// cache is kept, so the old hierarchy's handles are the last strong ones.
#[allow(clippy::too_many_arguments)]
fn sync_environment(
    mut commands: Commands,
    settings: Res<EnvironmentSettings>,
    mut state: ResMut<EnvironmentState>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    roots: Query<(), With<EnvironmentRoot>>,
    server: Option<Res<AssetServer>>,
    images: Option<Res<Assets<Image>>>,
) {
    let missing = state.root.is_some_and(|root| roots.get(root).is_err());
    let kind = if settings.suppressed {
        EnvironmentKind::Disabled
    } else {
        settings.kind
    };
    let wanted = kind != EnvironmentKind::Disabled;
    if state.kind != kind || (wanted && state.root.is_none()) || missing {
        if let Some(root) = state.root.take()
            && let Ok(mut entity) = commands.get_entity(root)
        {
            entity.despawn();
        }
        state.kind = kind;
        let textures = crate::rooms::RoomTextures {
            wall_art: if images.is_some()
                && matches!(
                    kind,
                    EnvironmentKind::RoseBedroom
                        | EnvironmentKind::VioletStudio
                        | EnvironmentKind::CyanLounge
                ) {
                server.as_ref().map(|server| {
                    let path = crate::bundled::assets_path().join("rooms/wave-art.png");
                    server.load(bevy::asset::AssetPath::from_path_buf(path))
                })
            } else {
                None
            },
        };
        if let Some(layout) = build_layout_textured(kind, &mut meshes, &mut materials, &textures) {
            let root = commands
                .spawn((
                    EnvironmentRoot { kind },
                    Name::new(format!("Environment: {}", kind.label())),
                    Transform::default(),
                    Visibility::Inherited,
                ))
                .with_children(|parent| {
                    for part in layout.parts {
                        let mut entity = parent.spawn((
                            Mesh3d(part.mesh),
                            crate::shaders::ExcludeShaderPreview,
                            MeshMaterial3d(part.material),
                            part.transform,
                        ));
                        if !matches!(
                            kind,
                            EnvironmentKind::RoseBedroom
                                | EnvironmentKind::VioletStudio
                                | EnvironmentKind::CyanLounge
                        ) {
                            entity.insert(NotShadowCaster);
                        }
                    }
                    for (light, transform) in layout.lights {
                        parent.spawn((light, transform));
                    }
                })
                .id();
            state.root = Some(root);
        }
    }
}

struct Part {
    mesh: Handle<Mesh>,
    material: Handle<StandardMaterial>,
    transform: Transform,
}

struct Layout {
    parts: Vec<Part>,
    lights: Vec<(PointLight, Transform)>,
}

/// Unit-sized shared meshes (1 m bounding box) scaled per part.
struct Builder<'a> {
    materials: &'a mut Assets<StandardMaterial>,
    cube: Handle<Mesh>,
    cylinder: Handle<Mesh>,
    sphere: Handle<Mesh>,
    cone: Handle<Mesh>,
    layout: Layout,
}

impl<'a> Builder<'a> {
    fn new(meshes: &mut Assets<Mesh>, materials: &'a mut Assets<StandardMaterial>) -> Self {
        Self {
            materials,
            cube: meshes.add(Cuboid::new(1.0, 1.0, 1.0)),
            cylinder: meshes.add(Cylinder::new(0.5, 1.0).mesh().resolution(10)),
            sphere: meshes.add(
                Sphere::new(0.5)
                    .mesh()
                    .ico(1)
                    .unwrap_or_else(|_| Sphere::new(0.5).mesh().uv(8, 6)),
            ),
            cone: meshes.add(Cone::new(0.5, 1.0).mesh().resolution(10)),
            layout: Layout {
                parts: Vec::new(),
                lights: Vec::new(),
            },
        }
    }

    fn solid(&mut self, color: Color, roughness: f32) -> Handle<StandardMaterial> {
        self.materials.add(StandardMaterial {
            base_color: color,
            perceptual_roughness: roughness,
            ..default()
        })
    }

    fn glow(&mut self, color: Color, strength: f32) -> Handle<StandardMaterial> {
        self.materials.add(StandardMaterial {
            base_color: color,
            emissive: color.to_linear() * strength,
            ..default()
        })
    }

    fn sky(&mut self, color: Color) -> Handle<StandardMaterial> {
        self.materials.add(StandardMaterial {
            base_color: color,
            unlit: true,
            ..default()
        })
    }

    fn put(
        &mut self,
        mesh: Handle<Mesh>,
        material: &Handle<StandardMaterial>,
        transform: Transform,
    ) {
        self.layout.parts.push(Part {
            mesh,
            material: material.clone(),
            transform,
        });
    }

    fn cube(&mut self, material: &Handle<StandardMaterial>, at: Vec3, size: Vec3) {
        self.put(
            self.cube.clone(),
            material,
            Transform::from_translation(at).with_scale(size),
        );
    }

    fn cube_turned(&mut self, material: &Handle<StandardMaterial>, at: Vec3, size: Vec3, yaw: f32) {
        let transform = Transform::from_translation(at)
            .with_rotation(Quat::from_rotation_y(yaw))
            .with_scale(size);
        self.put(self.cube.clone(), material, transform);
    }

    /// `size` is (diameter, height, diameter).
    fn cylinder(&mut self, material: &Handle<StandardMaterial>, at: Vec3, size: Vec3) {
        self.put(
            self.cylinder.clone(),
            material,
            Transform::from_translation(at).with_scale(size),
        );
    }

    /// Cylinder turned to face +Z (toward the camera).
    fn disc(&mut self, material: &Handle<StandardMaterial>, at: Vec3, diameter: f32, depth: f32) {
        let transform = Transform::from_translation(at)
            .with_rotation(Quat::from_rotation_x(std::f32::consts::FRAC_PI_2))
            .with_scale(Vec3::new(diameter, depth, diameter));
        self.put(self.cylinder.clone(), material, transform);
    }

    fn sphere(&mut self, material: &Handle<StandardMaterial>, at: Vec3, size: Vec3) {
        self.put(
            self.sphere.clone(),
            material,
            Transform::from_translation(at).with_scale(size),
        );
    }

    fn cone(&mut self, material: &Handle<StandardMaterial>, at: Vec3, size: Vec3) {
        self.put(
            self.cone.clone(),
            material,
            Transform::from_translation(at).with_scale(size),
        );
    }

    fn light(&mut self, color: Color, intensity: f32, range: f32, at: Vec3) {
        self.layout.lights.push((
            PointLight {
                color,
                intensity,
                range,
                shadow_maps_enabled: false,
                ..default()
            },
            Transform::from_translation(at),
        ));
    }
}

/// Small deterministic generator so scatter layouts are identical every run.
struct Scatter(u32);

impl Scatter {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (self.0 >> 8) as f32 / (1u32 << 24) as f32
    }

    fn range(&mut self, min: f32, max: f32) -> f32 {
        min + (max - min) * self.next()
    }
}

#[cfg(test)]
fn build_layout(
    kind: EnvironmentKind,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) -> Option<Layout> {
    build_layout_textured(
        kind,
        meshes,
        materials,
        &crate::rooms::RoomTextures::default(),
    )
}

fn build_layout_textured(
    kind: EnvironmentKind,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    textures: &crate::rooms::RoomTextures,
) -> Option<Layout> {
    let theme = match kind {
        EnvironmentKind::RoseBedroom => Some(crate::rooms::RoomTheme::Rose),
        EnvironmentKind::VioletStudio => Some(crate::rooms::RoomTheme::Violet),
        EnvironmentKind::CyanLounge => Some(crate::rooms::RoomTheme::Cyan),
        _ => None,
    };
    if let Some(theme) = theme {
        let room = crate::rooms::build_room_textured(theme, meshes, materials, textures);
        return Some(Layout {
            parts: room
                .parts
                .into_iter()
                .map(|p| Part {
                    mesh: p.mesh,
                    material: p.material,
                    transform: p.transform,
                })
                .collect(),
            lights: room.lights,
        });
    }
    let build = match kind {
        EnvironmentKind::Disabled => return None,
        EnvironmentKind::CozyRoom => cozy_room,
        EnvironmentKind::MoonlitGarden => moonlit_garden,
        EnvironmentKind::NeonRooftop => neon_rooftop,
        EnvironmentKind::RoseBedroom
        | EnvironmentKind::VioletStudio
        | EnvironmentKind::CyanLounge => unreachable!(),
    };
    let mut builder = Builder::new(meshes, materials);
    build(&mut builder);
    Some(builder.layout)
}

fn cozy_room(b: &mut Builder) {
    let oak = b.solid(Color::srgb(0.55, 0.36, 0.22), 0.8);
    let walnut = b.solid(Color::srgb(0.42, 0.27, 0.16), 0.8);
    let dark_wood = b.solid(Color::srgb(0.28, 0.18, 0.11), 0.75);
    let wall = b.solid(Color::srgb(0.86, 0.76, 0.66), 0.95);
    let wainscot = b.solid(Color::srgb(0.62, 0.45, 0.36), 0.9);
    let rug = b.solid(Color::srgb(0.66, 0.30, 0.30), 1.0);
    let fabric = b.solid(Color::srgb(0.38, 0.48, 0.58), 1.0);
    let cushion = b.solid(Color::srgb(0.90, 0.80, 0.62), 1.0);
    let leaf = b.solid(Color::srgb(0.30, 0.52, 0.30), 0.9);
    let clay = b.solid(Color::srgb(0.72, 0.42, 0.30), 0.9);
    let dusk = b.glow(Color::srgb(0.36, 0.42, 0.78), 1.5);
    let lamp = b.glow(Color::srgb(1.0, 0.80, 0.52), 4.0);
    let bulb = b.glow(Color::srgb(1.0, 0.86, 0.60), 6.0);
    let books = [
        b.solid(Color::srgb(0.72, 0.26, 0.24), 0.8),
        b.solid(Color::srgb(0.24, 0.40, 0.56), 0.8),
        b.solid(Color::srgb(0.86, 0.70, 0.38), 0.8),
        b.solid(Color::srgb(0.32, 0.50, 0.38), 0.8),
    ];

    // Plank floor (top face at y = 0) and a rug under the avatar.
    for i in 0..8 {
        let material = if i % 2 == 0 { &oak } else { &walnut };
        b.cube(
            material,
            Vec3::new(-3.5 + i as f32, -0.025, -0.25),
            Vec3::new(0.98, 0.05, 6.5),
        );
    }
    b.cylinder(
        &rug,
        Vec3::new(0.0, 0.006, -0.6),
        Vec3::new(3.0, 0.012, 2.2),
    );

    // Back and partial side walls; no ceiling over the avatar.
    b.cube(&wall, Vec3::new(0.0, 1.6, -3.6), Vec3::new(8.2, 3.2, 0.2));
    b.cube(
        &wainscot,
        Vec3::new(0.0, 0.45, -3.48),
        Vec3::new(8.2, 0.9, 0.05),
    );
    for x in [-4.0, 4.0] {
        b.cube(&wall, Vec3::new(x, 1.6, -1.6), Vec3::new(0.2, 3.2, 4.2));
    }

    // Window with a dusk-blue pane, frame, mullions and curtains.
    let window = Vec3::new(1.5, 1.8, -3.47);
    b.cube(&dusk, window, Vec3::new(1.6, 1.2, 0.04));
    for dy in [-0.64, 0.64] {
        b.cube(
            &dark_wood,
            window + Vec3::new(0.0, dy, 0.02),
            Vec3::new(1.8, 0.08, 0.08),
        );
    }
    for dx in [-0.84, 0.0, 0.84] {
        b.cube(
            &dark_wood,
            window + Vec3::new(dx, 0.0, 0.02),
            Vec3::new(0.08, 1.3, 0.08),
        );
    }
    b.cube(
        &dark_wood,
        window + Vec3::new(0.0, 0.0, 0.02),
        Vec3::new(1.7, 0.05, 0.06),
    );
    for dx in [-1.05, 1.05] {
        b.cube(
            &fabric,
            window + Vec3::new(dx, -0.05, 0.05),
            Vec3::new(0.35, 1.6, 0.06),
        );
    }

    // Framed picture.
    b.cube(
        &dark_wood,
        Vec3::new(-0.6, 2.0, -3.48),
        Vec3::new(0.9, 0.6, 0.04),
    );
    b.cube(
        &cushion,
        Vec3::new(-0.6, 2.0, -3.455),
        Vec3::new(0.78, 0.48, 0.02),
    );
    b.sphere(
        &clay,
        Vec3::new(-0.75, 1.95, -3.44),
        Vec3::new(0.22, 0.22, 0.02),
    );

    // Bookshelf.
    b.cube(
        &dark_wood,
        Vec3::new(-2.5, 1.0, -3.45),
        Vec3::new(1.4, 2.0, 0.05),
    );
    for x in [-3.18, -1.82] {
        b.cube(
            &dark_wood,
            Vec3::new(x, 1.0, -3.25),
            Vec3::new(0.05, 2.0, 0.45),
        );
    }
    for y in [0.02, 0.5, 1.0, 1.5, 2.0] {
        b.cube(
            &walnut,
            Vec3::new(-2.5, y, -3.25),
            Vec3::new(1.36, 0.04, 0.45),
        );
    }
    let mut scatter = Scatter(7);
    for shelf in [0.04, 0.52, 1.02, 1.52] {
        let mut x = -3.12;
        while x < -2.0 {
            let width = scatter.range(0.07, 0.12);
            let height = scatter.range(0.24, 0.36);
            let book = &books[(scatter.next() * books.len() as f32) as usize % books.len()];
            b.cube(
                book,
                Vec3::new(x + width * 0.5, shelf + height * 0.5, -3.2),
                Vec3::new(width * 0.92, height, 0.26),
            );
            x += width + scatter.range(0.0, 0.05);
        }
    }

    // Sofa with cushions.
    b.cube(
        &fabric,
        Vec3::new(2.6, 0.22, -2.9),
        Vec3::new(1.9, 0.44, 0.8),
    );
    b.cube(
        &fabric,
        Vec3::new(2.6, 0.65, -3.25),
        Vec3::new(1.9, 0.7, 0.2),
    );
    for x in [1.75, 3.45] {
        b.cube(&fabric, Vec3::new(x, 0.42, -2.9), Vec3::new(0.2, 0.45, 0.8));
    }
    for x in [2.15, 3.05] {
        b.cube(
            &cushion,
            Vec3::new(x, 0.5, -2.85),
            Vec3::new(0.8, 0.14, 0.65),
        );
    }
    b.sphere(
        &rug,
        Vec3::new(1.98, 0.7, -3.05),
        Vec3::new(0.35, 0.3, 0.15),
    );

    // Side table with a mug.
    b.cube(
        &walnut,
        Vec3::new(1.25, 0.25, -3.0),
        Vec3::new(0.4, 0.5, 0.4),
    );
    b.cylinder(
        &cushion,
        Vec3::new(1.2, 0.56, -3.0),
        Vec3::new(0.09, 0.12, 0.09),
    );

    // Floor lamp.
    b.cylinder(
        &dark_wood,
        Vec3::new(-1.5, 0.02, -2.9),
        Vec3::new(0.35, 0.04, 0.35),
    );
    b.cylinder(
        &dark_wood,
        Vec3::new(-1.5, 0.8, -2.9),
        Vec3::new(0.04, 1.6, 0.04),
    );
    b.cylinder(
        &lamp,
        Vec3::new(-1.5, 1.7, -2.9),
        Vec3::new(0.45, 0.35, 0.45),
    );
    b.light(
        Color::srgb(1.0, 0.78, 0.5),
        120_000.0,
        6.0,
        Vec3::new(-1.5, 1.6, -2.6),
    );

    // Potted plant.
    b.cylinder(&clay, Vec3::new(-3.4, 0.2, -2.0), Vec3::new(0.4, 0.4, 0.4));
    b.sphere(&leaf, Vec3::new(-3.4, 0.65, -2.0), Vec3::splat(0.6));
    b.sphere(&leaf, Vec3::new(-3.3, 0.95, -2.05), Vec3::splat(0.4));

    // Sagging string lights along the top of the back wall.
    for i in 0..13 {
        let t = i as f32 / 12.0;
        let sag = (t * std::f32::consts::TAU * 2.0).sin().abs() * 0.12;
        b.sphere(
            &bulb,
            Vec3::new(-3.6 + 7.2 * t, 2.95 - sag, -3.45),
            Vec3::splat(0.07),
        );
    }
    b.light(
        Color::srgb(1.0, 0.85, 0.65),
        40_000.0,
        5.0,
        Vec3::new(0.0, 2.6, -3.0),
    );
}

fn moonlit_garden(b: &mut Builder) {
    let grass = b.solid(Color::srgb(0.10, 0.20, 0.14), 1.0);
    let hedge = b.solid(Color::srgb(0.08, 0.22, 0.14), 0.95);
    let hills = b.solid(Color::srgb(0.07, 0.10, 0.16), 1.0);
    let stone = b.solid(Color::srgb(0.46, 0.48, 0.52), 0.9);
    let bark = b.solid(Color::srgb(0.22, 0.15, 0.11), 0.95);
    let pine = b.solid(Color::srgb(0.06, 0.18, 0.15), 0.95);
    let wood = b.solid(Color::srgb(0.35, 0.26, 0.20), 0.85);
    let iron = b.solid(Color::srgb(0.12, 0.12, 0.14), 0.6);
    let water = b.materials.add(StandardMaterial {
        base_color: Color::srgb(0.05, 0.09, 0.20),
        perceptual_roughness: 0.08,
        metallic: 0.6,
        ..default()
    });
    let pad = b.solid(Color::srgb(0.18, 0.38, 0.20), 0.9);
    let flowers = [
        b.solid(Color::srgb(0.80, 0.62, 0.92), 0.9),
        b.solid(Color::srgb(0.95, 0.92, 0.85), 0.9),
        b.solid(Color::srgb(0.55, 0.62, 0.95), 0.9),
    ];
    let night = b.sky(Color::srgb(0.03, 0.05, 0.12));
    let moon = b.glow(Color::srgb(0.92, 0.94, 1.0), 8.0);
    let star = b.glow(Color::srgb(0.85, 0.90, 1.0), 6.0);
    let lantern = b.glow(Color::srgb(1.0, 0.78, 0.45), 6.0);
    let firefly = b.glow(Color::srgb(0.80, 1.0, 0.45), 8.0);

    b.cube(
        &grass,
        Vec3::new(0.0, -0.05, -10.0),
        Vec3::new(40.0, 0.1, 40.0),
    );
    b.cube(
        &night,
        Vec3::new(0.0, 12.0, -28.0),
        Vec3::new(80.0, 40.0, 0.5),
    );
    b.sphere(&moon, Vec3::new(-7.0, 10.0, -26.0), Vec3::splat(3.0));
    let mut scatter = Scatter(42);
    for _ in 0..40 {
        let at = Vec3::new(scatter.range(-30.0, 30.0), scatter.range(5.0, 22.0), -27.6);
        if at.distance(Vec3::new(-7.0, 10.0, -27.6)) < 3.0 {
            continue;
        }
        b.sphere(&star, at, Vec3::splat(scatter.range(0.08, 0.2)));
    }
    for (at, size) in [
        (Vec3::new(-12.0, 0.0, -20.0), Vec3::new(20.0, 6.0, 8.0)),
        (Vec3::new(8.0, -1.0, -22.0), Vec3::new(26.0, 8.0, 10.0)),
        (Vec3::new(-1.0, -1.5, -24.0), Vec3::new(18.0, 7.0, 8.0)),
    ] {
        b.sphere(&hills, at, size);
    }

    // Stepping-stone path leading away from the avatar.
    for i in 0..8 {
        let z = -1.6 - i as f32 * 0.85;
        let x = (i as f32 * 0.9).sin() * 0.35;
        b.cylinder(&stone, Vec3::new(x, 0.011, z), Vec3::new(0.55, 0.02, 0.45));
    }

    // Hedges framing the path, with flowers along their base.
    for x in [-3.2, 3.2] {
        b.cube(&hedge, Vec3::new(x, 0.45, -4.5), Vec3::new(0.9, 0.9, 5.0));
        for i in 0..6 {
            let flower = &flowers[i % flowers.len()];
            let z = -2.3 - i as f32 * 0.8;
            b.sphere(flower, Vec3::new(x * 0.8, 0.12, z), Vec3::splat(0.14));
        }
    }

    // Trellis arch over the path.
    for x in [-0.9, 0.9] {
        b.cube(&wood, Vec3::new(x, 1.1, -6.0), Vec3::new(0.12, 2.2, 0.12));
    }
    b.cube(
        &wood,
        Vec3::new(0.0, 2.25, -6.0),
        Vec3::new(2.0, 0.12, 0.12),
    );
    for i in 0..7 {
        let x = -0.9 + i as f32 * 0.3;
        b.sphere(
            &flowers[i % flowers.len()],
            Vec3::new(x, 2.32, -5.94),
            Vec3::splat(0.16),
        );
    }

    // Low-poly pines.
    for (x, z, s) in [
        (-5.0, -9.0, 1.1),
        (5.5, -10.0, 1.2),
        (-8.0, -14.0, 1.5),
        (9.0, -15.0, 1.6),
        (-3.5, -16.0, 1.3),
        (3.0, -18.0, 1.8),
    ] {
        b.cylinder(
            &bark,
            Vec3::new(x, 0.6 * s, z),
            Vec3::new(0.3, 1.2, 0.3) * s,
        );
        b.cone(
            &pine,
            Vec3::new(x, 1.6 * s, z),
            Vec3::new(2.0, 1.6, 2.0) * s,
        );
        b.cone(
            &pine,
            Vec3::new(x, 2.5 * s, z),
            Vec3::new(1.5, 1.4, 1.5) * s,
        );
        b.cone(
            &pine,
            Vec3::new(x, 3.3 * s, z),
            Vec3::new(1.0, 1.2, 1.0) * s,
        );
    }

    // Lantern posts.
    for at in [Vec3::new(-1.8, 0.0, -3.2), Vec3::new(1.8, 0.0, -3.6)] {
        b.cylinder(&iron, at + Vec3::Y * 0.6, Vec3::new(0.08, 1.2, 0.08));
        b.cube(&iron, at + Vec3::Y * 1.33, Vec3::new(0.25, 0.05, 0.25));
        b.sphere(&lantern, at + Vec3::Y * 1.2, Vec3::splat(0.2));
        b.light(
            Color::srgb(1.0, 0.78, 0.5),
            50_000.0,
            5.0,
            at + Vec3::Y * 1.2,
        );
    }
    b.light(
        Color::srgb(0.55, 0.65, 1.0),
        150_000.0,
        14.0,
        Vec3::new(-4.0, 4.0, -8.0),
    );

    // Pond with lily pads.
    b.cylinder(
        &water,
        Vec3::new(3.0, 0.008, -2.6),
        Vec3::new(1.8, 0.016, 1.4),
    );
    for at in [Vec3::new(2.6, 0.02, -2.4), Vec3::new(3.4, 0.02, -2.9)] {
        b.cylinder(&pad, at, Vec3::new(0.3, 0.01, 0.3));
    }

    // Bench beside the path.
    b.cube(&wood, Vec3::new(2.0, 0.42, -5.2), Vec3::new(1.2, 0.06, 0.4));
    b.cube(
        &wood,
        Vec3::new(2.0, 0.75, -5.38),
        Vec3::new(1.2, 0.3, 0.05),
    );
    for x in [1.5, 2.5] {
        b.cube(&iron, Vec3::new(x, 0.2, -5.2), Vec3::new(0.06, 0.4, 0.36));
    }

    // Static fireflies.
    for _ in 0..14 {
        let at = Vec3::new(
            scatter.range(-4.0, 4.0),
            scatter.range(0.4, 1.6),
            scatter.range(-7.0, -2.2),
        );
        b.sphere(&firefly, at, Vec3::splat(0.04));
    }
}

fn neon_rooftop(b: &mut Builder) {
    let concrete = b.solid(Color::srgb(0.20, 0.20, 0.23), 0.9);
    let parapet = b.solid(Color::srgb(0.16, 0.16, 0.19), 0.9);
    let puddle = b.materials.add(StandardMaterial {
        base_color: Color::srgb(0.05, 0.05, 0.08),
        perceptual_roughness: 0.05,
        metallic: 0.8,
        ..default()
    });
    let metal = b.solid(Color::srgb(0.40, 0.42, 0.46), 0.5);
    let crate_wood = b.solid(Color::srgb(0.36, 0.28, 0.22), 0.9);
    let towers = [
        b.solid(Color::srgb(0.08, 0.08, 0.16), 0.8),
        b.solid(Color::srgb(0.12, 0.10, 0.20), 0.8),
        b.solid(Color::srgb(0.06, 0.10, 0.16), 0.8),
    ];
    let windows = [
        b.glow(Color::srgb(1.0, 0.72, 0.35), 3.0),
        b.glow(Color::srgb(0.30, 0.90, 1.0), 3.0),
        b.glow(Color::srgb(1.0, 0.30, 0.70), 3.0),
    ];
    let sky = b.sky(Color::srgb(0.07, 0.03, 0.14));
    let haze = b.sky(Color::srgb(0.30, 0.08, 0.30));
    let pink = b.glow(Color::srgb(1.0, 0.20, 0.65), 10.0);
    let cyan = b.glow(Color::srgb(0.15, 0.95, 1.0), 10.0);
    let beacon = b.glow(Color::srgb(1.0, 0.10, 0.10), 8.0);

    b.cube(
        &concrete,
        Vec3::new(0.0, -0.1, -3.0),
        Vec3::new(14.0, 0.2, 14.0),
    );
    b.cube(
        &puddle,
        Vec3::new(1.8, 0.003, -1.8),
        Vec3::new(1.2, 0.006, 0.8),
    );
    b.cube(
        &puddle,
        Vec3::new(-2.4, 0.003, -3.4),
        Vec3::new(0.9, 0.006, 1.3),
    );

    // Parapets with neon trim.
    b.cube(
        &parapet,
        Vec3::new(0.0, 0.5, -7.0),
        Vec3::new(14.0, 1.0, 0.3),
    );
    b.cube(
        &cyan,
        Vec3::new(0.0, 1.02, -6.86),
        Vec3::new(14.0, 0.04, 0.06),
    );
    for x in [-7.0, 7.0] {
        b.cube(&parapet, Vec3::new(x, 0.5, -3.25), Vec3::new(0.3, 1.0, 7.5));
    }
    b.cube(
        &pink,
        Vec3::new(-6.86, 1.02, -3.25),
        Vec3::new(0.06, 0.04, 7.5),
    );

    // Sky and distant haze.
    b.cube(
        &sky,
        Vec3::new(0.0, 10.0, -32.0),
        Vec3::new(100.0, 60.0, 0.5),
    );
    b.cube(
        &haze,
        Vec3::new(0.0, -2.0, -31.5),
        Vec3::new(100.0, 6.0, 0.3),
    );

    // Skyline rising from the street far below, with lit window strips.
    let mut scatter = Scatter(1337);
    let mut x = -24.0;
    while x < 24.0 {
        let width = scatter.range(3.0, 6.0);
        let top = scatter.range(2.0, 18.0);
        let z = scatter.range(-26.0, -14.0);
        let depth = scatter.range(3.0, 6.0);
        let base = -20.0;
        let tower = &towers[(scatter.next() * towers.len() as f32) as usize % towers.len()];
        let center = Vec3::new(x + width * 0.5, (top + base) * 0.5, z);
        b.cube(tower, center, Vec3::new(width, top - base, depth));
        let front = z + depth * 0.5 + 0.03;
        let strips = 2 + (scatter.next() * 3.0) as usize;
        for i in 0..strips {
            let window = &windows[(scatter.next() * windows.len() as f32) as usize % windows.len()];
            let y = top - 1.0 - i as f32 * scatter.range(1.2, 2.4);
            b.cube(
                window,
                Vec3::new(center.x, y, front),
                Vec3::new(width * 0.8, 0.12, 0.05),
            );
        }
        x += width + scatter.range(0.5, 2.0);
    }

    // Neon sign on the back-left of the roof.
    for x in [-4.8, -2.8] {
        b.cube(&metal, Vec3::new(x, 1.3, -6.2), Vec3::new(0.1, 2.6, 0.1));
    }
    b.cube(
        &parapet,
        Vec3::new(-3.8, 2.4, -6.25),
        Vec3::new(2.4, 1.0, 0.08),
    );
    // A glowing disc with a dark inset reads as a neon ring facing the camera.
    b.disc(&pink, Vec3::new(-4.4, 2.4, -6.18), 0.7, 0.05);
    b.disc(&parapet, Vec3::new(-4.4, 2.4, -6.15), 0.52, 0.06);
    b.cube(
        &cyan,
        Vec3::new(-3.4, 2.6, -6.18),
        Vec3::new(1.0, 0.1, 0.05),
    );
    b.cube(
        &cyan,
        Vec3::new(-3.5, 2.2, -6.18),
        Vec3::new(0.8, 0.1, 0.05),
    );
    b.light(
        Color::srgb(1.0, 0.3, 0.7),
        150_000.0,
        8.0,
        Vec3::new(-3.8, 2.4, -5.6),
    );

    // Water tank on stilts.
    for (dx, dz) in [(-0.6, -0.6), (0.6, -0.6), (-0.6, 0.6), (0.6, 0.6)] {
        b.cube(
            &metal,
            Vec3::new(4.6 + dx, 0.4, -5.5 + dz),
            Vec3::new(0.1, 0.8, 0.1),
        );
    }
    b.cylinder(
        &crate_wood,
        Vec3::new(4.6, 1.7, -5.5),
        Vec3::new(1.6, 1.8, 1.6),
    );
    b.cone(&metal, Vec3::new(4.6, 2.85, -5.5), Vec3::new(1.7, 0.5, 1.7));

    // Air-conditioning units.
    for at in [Vec3::new(1.6, 0.3, -6.2), Vec3::new(-0.4, 0.3, -6.3)] {
        b.cube(&metal, at, Vec3::new(0.8, 0.6, 0.6));
        b.cylinder(&parapet, at + Vec3::Y * 0.31, Vec3::new(0.5, 0.03, 0.5));
    }
    b.light(
        Color::srgb(0.3, 0.9, 1.0),
        100_000.0,
        8.0,
        Vec3::new(2.0, 2.5, -4.5),
    );

    // Antenna with a beacon.
    b.cylinder(
        &metal,
        Vec3::new(5.8, 2.5, -6.4),
        Vec3::new(0.05, 5.0, 0.05),
    );
    b.sphere(&beacon, Vec3::new(5.8, 5.05, -6.4), Vec3::splat(0.12));

    // Stacked crates.
    b.cube(&crate_wood, Vec3::new(-5.6, 0.3, -4.5), Vec3::splat(0.6));
    b.cube_turned(
        &crate_wood,
        Vec3::new(-5.6, 0.85, -4.4),
        Vec3::splat(0.5),
        0.3,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .init_asset::<Mesh>()
            .init_asset::<StandardMaterial>()
            .add_plugins(EnvironmentPlugin);
        app
    }

    fn select(app: &mut App, kind: EnvironmentKind) {
        app.world_mut().resource_mut::<EnvironmentSettings>().kind = kind;
        app.update();
    }

    fn roots(app: &mut App) -> Vec<Entity> {
        let mut query = app
            .world_mut()
            .query_filtered::<Entity, With<EnvironmentRoot>>();
        query.iter(app.world()).collect()
    }

    fn count<F: bevy::ecs::query::QueryFilter>(app: &mut App) -> usize {
        let mut query = app.world_mut().query_filtered::<(), F>();
        query.iter(app.world()).count()
    }

    #[test]
    fn switching_replaces_hierarchy_and_disabling_releases_assets() {
        let mut app = app();
        app.update();
        assert!(roots(&mut app).is_empty());

        select(&mut app, EnvironmentKind::CozyRoom);
        let first = roots(&mut app);
        assert_eq!(first.len(), 1);
        let parts = count::<With<Mesh3d>>(&mut app);
        assert!(parts > 0);
        for _ in 0..5 {
            app.update();
        }
        assert_eq!(roots(&mut app), first, "must not respawn while unchanged");
        assert_eq!(count::<With<Mesh3d>>(&mut app), parts);

        select(&mut app, EnvironmentKind::NeonRooftop);
        let second = roots(&mut app);
        assert_eq!(second.len(), 1);
        assert_ne!(second, first);
        assert!(app.world().get_entity(first[0]).is_err());

        for theme in [
            EnvironmentKind::RoseBedroom,
            EnvironmentKind::VioletStudio,
            EnvironmentKind::CyanLounge,
        ] {
            let before = roots(&mut app)[0];
            select(&mut app, theme);
            assert!(app.world().get_entity(before).is_err());
            let current = roots(&mut app);
            let parts = count::<With<Mesh3d>>(&mut app);
            assert!(parts > 0 && parts <= 1800);
            for _ in 0..3 {
                app.update();
            }
            assert_eq!(roots(&mut app), current);
            assert_eq!(count::<With<Mesh3d>>(&mut app), parts);
        }
        select(&mut app, EnvironmentKind::Disabled);
        for _ in 0..3 {
            app.update();
        }
        assert!(roots(&mut app).is_empty());
        assert_eq!(count::<With<Mesh3d>>(&mut app), 0);
        assert_eq!(count::<With<PointLight>>(&mut app), 0);
        assert_eq!(app.world().resource::<Assets<Mesh>>().len(), 0);
        assert_eq!(app.world().resource::<Assets<StandardMaterial>>().len(), 0);
    }

    #[test]
    fn suppression_releases_room_assets_and_restores_the_selected_room() {
        let mut app = app();
        select(&mut app, EnvironmentKind::MoonlitGarden);
        let root = roots(&mut app)[0];
        app.world_mut()
            .resource_mut::<EnvironmentSettings>()
            .suppressed = true;
        for _ in 0..4 {
            app.update();
        }
        assert!(roots(&mut app).is_empty());
        assert!(app.world().get_entity(root).is_err());
        assert_eq!(count::<With<Mesh3d>>(&mut app), 0);
        assert_eq!(count::<With<PointLight>>(&mut app), 0);
        assert_eq!(app.world().resource::<Assets<Mesh>>().len(), 0);
        assert_eq!(app.world().resource::<Assets<StandardMaterial>>().len(), 0);
        assert_eq!(
            app.world().resource::<EnvironmentSettings>().kind,
            EnvironmentKind::MoonlitGarden
        );
        // Even a changed selection stays unloaded until returning to Studio.
        select(&mut app, EnvironmentKind::CozyRoom);
        assert!(roots(&mut app).is_empty());
        assert_eq!(app.world().resource::<Assets<Mesh>>().len(), 0);
        app.world_mut()
            .resource_mut::<EnvironmentSettings>()
            .suppressed = false;
        app.update();
        let restored = roots(&mut app)[0];
        assert_eq!(
            app.world().get::<EnvironmentRoot>(restored).unwrap().kind,
            EnvironmentKind::CozyRoom
        );
        assert_eq!(
            app.world().get::<Visibility>(restored),
            Some(&Visibility::Inherited)
        );
        app.update();
        assert_eq!(roots(&mut app), vec![restored]);
    }

    #[test]
    fn externally_despawned_root_is_rebuilt_once() {
        let mut app = app();
        select(&mut app, EnvironmentKind::CozyRoom);
        let root = roots(&mut app)[0];
        app.world_mut().entity_mut(root).despawn();
        app.update();
        app.update();
        assert_eq!(roots(&mut app).len(), 1);
    }

    #[test]
    fn settings_default_and_round_trip() {
        let settings: EnvironmentSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(settings.kind, EnvironmentKind::Disabled);
        assert!(!settings.is_active());
        let json = serde_json::to_string(&EnvironmentSettings {
            kind: EnvironmentKind::MoonlitGarden,
            suppressed: true,
        })
        .unwrap();
        assert_eq!(json, r#"{"kind":"moonlit_garden"}"#);
        let back: EnvironmentSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.kind, EnvironmentKind::MoonlitGarden);
        assert!(!back.suppressed);
    }

    #[test]
    fn layouts_are_bounded_and_keep_the_avatar_clear() {
        for kind in EnvironmentKind::ALL {
            let mut meshes = Assets::<Mesh>::default();
            let mut materials = Assets::<StandardMaterial>::default();
            let Some(layout) = build_layout(kind, &mut meshes, &mut materials) else {
                assert_eq!(kind, EnvironmentKind::Disabled);
                continue;
            };
            assert!(
                layout.parts.len() <= 1800,
                "{kind:?}: {}",
                layout.parts.len()
            );
            assert!(layout.lights.len() <= 5);
            assert!(meshes.len() <= 12 && materials.len() <= 48);
            for part in &layout.parts {
                let mut min = Vec3::splat(f32::INFINITY);
                let mut max = Vec3::splat(f32::NEG_INFINITY);
                for corner in [-0.5, 0.5]
                    .into_iter()
                    .flat_map(|x| [-0.5, 0.5].map(|y| (x, y)))
                    .flat_map(|(x, y)| [-0.5, 0.5].map(|z| Vec3::new(x, y, z)))
                {
                    let point = part.transform.transform_point(corner);
                    min = min.min(point);
                    max = max.max(point);
                }
                // Floor-level dressing (planks, rug, stones) may pass under the avatar.
                if max.y <= 0.06 {
                    continue;
                }
                let foreground_limit = if matches!(
                    kind,
                    EnvironmentKind::RoseBedroom
                        | EnvironmentKind::VioletStudio
                        | EnvironmentKind::CyanLounge
                ) {
                    3.0
                } else {
                    0.8
                };
                assert!(
                    max.z < foreground_limit,
                    "{kind:?}: foreground part at {min:?}..{max:?}"
                );
                let blocks_avatar = min.x < 1.0 && max.x > -1.0 && min.z < 1.0 && max.z > -1.0;
                assert!(
                    !blocks_avatar,
                    "{kind:?}: part near avatar at {min:?}..{max:?}"
                );
            }
        }
    }
}
