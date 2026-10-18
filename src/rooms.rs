//! Furnished, open-front rooms. Shared meshes and a small palette keep scene cost bounded.
use bevy::{
    asset::RenderAssetUsages,
    mesh::{Indices, PrimitiveTopology},
    prelude::*,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoomTheme {
    Rose,
    Violet,
    Cyan,
}
#[derive(Default)]
pub struct RoomTextures {
    pub wall_art: Option<Handle<Image>>,
}
pub struct RoomPart {
    pub mesh: Handle<Mesh>,
    pub material: Handle<StandardMaterial>,
    pub transform: Transform,
}
pub struct RoomLayout {
    pub parts: Vec<RoomPart>,
    pub lights: Vec<(PointLight, Transform)>,
}

/// A rounded unit cube, with curved bevel normals rather than overlapping primitives.
fn rounded_box() -> Mesh {
    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut uvs = Vec::new();
    let mut indices = Vec::new();
    // Each face's tangents have cross product pointing outwards.
    for (normal, u, v) in [
        (Vec3::X, Vec3::NEG_Z, Vec3::Y),
        (Vec3::NEG_X, Vec3::Z, Vec3::Y),
        (Vec3::Y, Vec3::X, Vec3::NEG_Z),
        (Vec3::NEG_Y, Vec3::X, Vec3::Z),
        (Vec3::Z, Vec3::X, Vec3::Y),
        (Vec3::NEG_Z, Vec3::NEG_X, Vec3::Y),
    ] {
        let base = positions.len() as u32;
        // Dense samples at the bevels; broad planar center remains a single span.
        let coords = [
            -0.5, -0.485, -0.46, -0.425, -0.38, 0.38, 0.425, 0.46, 0.485, 0.5,
        ];
        for y in coords {
            for x in coords {
                let p = normal * 0.5 + u * x + v * y;
                let center = p.clamp(Vec3::splat(-0.38), Vec3::splat(0.38));
                let n = (p - center).normalize();
                positions.push((center + n * 0.12).to_array());
                normals.push(n.to_array());
                uvs.push([x + 0.5, y + 0.5]);
            }
        }
        for y in 0..9 {
            for x in 0..9 {
                let a = base + y * 10 + x;
                indices.extend_from_slice(&[a, a + 1, a + 11, a, a + 11, a + 10]);
            }
        }
    }
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
    .with_inserted_indices(Indices::U32(indices))
}
struct Builder {
    mesh: [Handle<Mesh>; 6],
    layout: RoomLayout,
}
impl Builder {
    fn new(meshes: &mut Assets<Mesh>) -> Self {
        Self {
            mesh: [
                meshes.add(Cuboid::new(1., 1., 1.)),
                meshes.add(rounded_box()),
                meshes.add(Cylinder::new(0.5, 1.).mesh().resolution(32)),
                meshes.add(Sphere::new(0.5).mesh().uv(24, 16)),
                meshes.add(Cylinder::new(0.5, 1.).mesh().resolution(6)),
                meshes.add(Rectangle::new(1., 1.)),
            ],
            layout: RoomLayout {
                parts: Vec::new(),
                lights: Vec::new(),
            },
        }
    }
    fn part(
        &mut self,
        shape: usize,
        mat: &Handle<StandardMaterial>,
        xyz: [f32; 3],
        size: [f32; 3],
        rot: Quat,
    ) {
        self.layout.parts.push(RoomPart {
            mesh: self.mesh[shape].clone(),
            material: mat.clone(),
            transform: Transform::from_translation(Vec3::from_array(xyz))
                .with_scale(Vec3::from_array(size))
                .with_rotation(rot),
        });
    }
    fn boxy(&mut self, m: &Handle<StandardMaterial>, p: [f32; 3], s: [f32; 3]) {
        self.part(0, m, p, s, Quat::IDENTITY);
    }
    fn soft(&mut self, m: &Handle<StandardMaterial>, p: [f32; 3], s: [f32; 3]) {
        self.part(1, m, p, s, Quat::IDENTITY);
    }
    fn cyl(&mut self, m: &Handle<StandardMaterial>, p: [f32; 3], s: [f32; 3]) {
        self.part(2, m, p, s, Quat::IDENTITY);
    }
    fn ball(&mut self, m: &Handle<StandardMaterial>, p: [f32; 3], s: [f32; 3]) {
        self.part(3, m, p, s, Quat::IDENTITY);
    }
    fn bar(&mut self, m: &Handle<StandardMaterial>, a: Vec3, z: Vec3, width: f32) {
        let d = z - a;
        self.part(
            2,
            m,
            ((a + z) * 0.5).to_array(),
            [width, d.length(), width],
            Quat::from_rotation_arc(Vec3::Y, d.normalize()),
        );
    }
    fn light(&mut self, color: Color, p: [f32; 3], power: f32) {
        self.layout.lights.push((
            PointLight {
                color,
                intensity: power,
                range: 12.,
                shadow_maps_enabled: false,
                ..default()
            },
            Transform::from_translation(Vec3::from_array(p)),
        ));
    }
}
struct Palette {
    wall: Handle<StandardMaterial>,
    trim: Handle<StandardMaterial>,
    floor: Handle<StandardMaterial>,
    seam: Handle<StandardMaterial>,
    wood: Handle<StandardMaterial>,
    upholstery: Handle<StandardMaterial>,
    pale: Handle<StandardMaterial>,
    accent: Handle<StandardMaterial>,
    dark: Handle<StandardMaterial>,
    metal: Handle<StandardMaterial>,
    glow: Handle<StandardMaterial>,
    glow2: Handle<StandardMaterial>,
    sheer: Handle<StandardMaterial>,
    screen: Handle<StandardMaterial>,
    glass: Handle<StandardMaterial>,
    paper: Handle<StandardMaterial>,
    colors: Vec<Handle<StandardMaterial>>,
}
fn material(
    m: &mut Assets<StandardMaterial>,
    rgb: [f32; 3],
    rough: f32,
) -> Handle<StandardMaterial> {
    m.add(StandardMaterial {
        base_color: Color::srgb(rgb[0], rgb[1], rgb[2]),
        perceptual_roughness: rough,
        ..default()
    })
}
fn palette(t: RoomTheme, m: &mut Assets<StandardMaterial>) -> Palette {
    let rose = t == RoomTheme::Rose;
    let cyan = t == RoomTheme::Cyan;
    let accent = if rose {
        [0.83, 0.40, 0.53]
    } else if cyan {
        [0.08, 0.76, 0.83]
    } else {
        [0.55, 0.24, 0.91]
    };
    let glow = m.add(StandardMaterial {
        base_color: Color::srgb(accent[0], accent[1], accent[2]),
        emissive: LinearRgba::rgb(accent[0] * 5., accent[1] * 5., accent[2] * 5.),
        ..default()
    });
    // Secondary emissive: warm fairy-light gold, hot gamer pink, or cool lounge blue.
    let second = if rose {
        [1.0, 0.74, 0.46]
    } else if cyan {
        [0.34, 0.52, 1.0]
    } else {
        [1.0, 0.22, 0.6]
    };
    let glow2 = m.add(StandardMaterial {
        base_color: Color::srgb(second[0], second[1], second[2]),
        emissive: LinearRgba::rgb(second[0] * 6., second[1] * 6., second[2] * 6.),
        ..default()
    });
    Palette {
        wall: material(
            m,
            if rose {
                [0.78, 0.57, 0.61]
            } else {
                [0.105, 0.115, 0.18]
            },
            0.9,
        ),
        trim: material(
            m,
            if rose {
                [0.96, 0.85, 0.81]
            } else {
                [0.23, 0.24, 0.32]
            },
            0.7,
        ),
        floor: material(
            m,
            if rose {
                [0.54, 0.36, 0.29]
            } else {
                [0.17, 0.20, 0.25]
            },
            0.65,
        ),
        seam: material(m, [0.065, 0.055, 0.07], 0.9),
        wood: material(
            m,
            if rose {
                [0.91, 0.77, 0.67]
            } else {
                [0.21, 0.14, 0.25]
            },
            0.5,
        ),
        upholstery: material(
            m,
            if rose {
                [0.79, 0.43, 0.53]
            } else {
                [0.19, 0.19, 0.31]
            },
            0.95,
        ),
        pale: material(
            m,
            if rose {
                [0.97, 0.84, 0.84]
            } else {
                [0.63, 0.64, 0.76]
            },
            0.95,
        ),
        accent: material(m, accent, 0.65),
        dark: material(m, [0.018, 0.024, 0.045], 0.55),
        metal: m.add(StandardMaterial {
            base_color: Color::srgb(0.67, 0.53, 0.36),
            metallic: 0.78,
            perceptual_roughness: 0.26,
            ..default()
        }),
        glow,
        glow2,
        sheer: m.add(StandardMaterial {
            base_color: Color::srgba(0.99, 0.86, 0.9, 0.5),
            alpha_mode: AlphaMode::Blend,
            perceptual_roughness: 0.95,
            double_sided: true,
            cull_mode: None,
            ..default()
        }),
        screen: m.add(StandardMaterial {
            base_color: Color::srgb(0.10, 0.18, 0.28),
            emissive: LinearRgba::rgb(0.10, 0.35, 0.55),
            ..default()
        }),
        glass: m.add(StandardMaterial {
            base_color: Color::srgba(0.62, 0.85, 0.94, 0.36),
            alpha_mode: AlphaMode::Blend,
            perceptual_roughness: 0.12,
            ..default()
        }),
        paper: material(m, [0.88, 0.85, 0.76], 0.9),
        colors: [
            [0.68, 0.29, 0.42],
            [0.25, 0.53, 0.63],
            [0.61, 0.47, 0.76],
            [0.83, 0.66, 0.38],
            [0.32, 0.39, 0.55],
        ]
        .into_iter()
        .map(|c| material(m, c, 0.8))
        .collect(),
    }
}
fn architecture(b: &mut Builder, p: &Palette, t: RoomTheme) {
    b.boxy(&p.floor, [0., -0.08, -0.5], [8., 0.14, 6.]);
    // Staggered plank joints and long narrow grain seams.
    for row in 0..16 {
        let x = -3.75 + row as f32 * 0.5;
        b.boxy(&p.seam, [x - 0.245, 0.001, -0.5], [0.009, 0.004, 6.]);
        for joint in 0..3 {
            let z = -3.45 + joint as f32 * 2. + (row % 2) as f32;
            if z < 2.45 {
                b.boxy(&p.seam, [x, 0.002, z], [0.49, 0.004, 0.008]);
            }
        }
    }
    b.boxy(&p.wall, [0., 1.8, -3.55], [8., 3.6, 0.15]);
    for x in [-4., 4.] {
        b.boxy(&p.wall, [x, 1.8, -1.25], [0.15, 3.6, 4.6]);
        b.boxy(&p.trim, [x * 0.98, 0.12, -1.25], [0.12, 0.24, 4.6]);
        b.boxy(&p.trim, [x * 0.98, 3.38, -1.25], [0.15, 0.16, 4.6]);
    }
    for y in [0.12, 3.34, 3.46] {
        b.boxy(&p.trim, [0., y, -3.42], [8., 0.10, 0.15]);
    }
    // Rear coffered ceiling leaves the foreground open to camera and key light.
    b.boxy(&p.wall, [0., 3.58, -2.5], [8., 0.12, 2.1]);
    for x in [-3.85, 0., 3.85] {
        b.boxy(&p.trim, [x, 3.46, -2.5], [0.10, 0.1, 2.]);
    }
    if t != RoomTheme::Rose {
        // Neon baseboard reads the back-wall silhouette in the dark rooms.
        b.soft(&p.glow2, [0., 0.2, -3.46], [7.8, 0.02, 0.02]);
        for z in [-3.30, -1.52] {
            b.soft(&p.glow, [0., 3.43, z], [7.65, 0.025, 0.025]);
        }
        for x in [-3.8, 3.8] {
            b.soft(&p.glow, [x, 3.43, -2.42], [0.025, 0.025, 1.8]);
        }
    } else {
        b.soft(&p.glow2, [0., 3.43, -3.30], [7.65, 0.025, 0.025]);
        for x in [-3., -1., 1., 3.] {
            for dx in [-0.65, 0.65] {
                b.boxy(&p.trim, [x + dx, 1.8, -3.44], [0.035, 2.5, 0.025]);
            }
            for y in [0.55, 3.05] {
                b.boxy(&p.trim, [x, y, -3.44], [1.33, 0.035, 0.025]);
            }
        }
    }
    b.soft(&p.pale, [0., 0.025, 0.0], [3.2, 0.035, 2.35]);
    for x in [-1.48, 1.48] {
        b.soft(&p.accent, [x, 0.047, 0.], [0.025, 0.009, 2.15]);
    }
}
fn books(b: &mut Builder, p: &Palette, x: f32, y: f32, z: f32, count: usize) {
    for i in 0..count {
        let h = 0.22 + (i % 4) as f32 * 0.035;
        let xx = x + i as f32 * 0.077;
        b.soft(
            &p.colors[i % p.colors.len()],
            [xx, y + h / 2., z],
            [0.063, h, 0.18],
        );
        for yy in [y + 0.04, y + h - 0.035] {
            b.boxy(&p.metal, [xx, yy, z + 0.093], [0.041, 0.008, 0.006]);
        }
        b.boxy(&p.paper, [xx, y + h - 0.008, z], [0.049, 0.007, 0.15]);
    }
}
fn shelves(b: &mut Builder, p: &Palette, x: f32) {
    for xx in [x - 0.53, x + 0.53] {
        b.soft(&p.wood, [xx, 1.28, -3.12], [0.08, 2.5, 0.47]);
    }
    b.boxy(&p.dark, [x, 1.28, -3.34], [1.05, 2.5, 0.045]);
    for row in 0..5 {
        let y = 0.10 + row as f32 * 0.49;
        b.soft(&p.wood, [x, y, -3.12], [1.14, 0.055, 0.48]);
        if row % 2 == 0 {
            books(b, p, x - 0.41, y + 0.03, -3.10, 9);
        } else {
            for offset in [-0.28, 0.27] {
                let xx = x + offset;
                b.cyl(&p.metal, [xx, y + 0.065, -3.10], [0.18, 0.07, 0.18]);
                b.ball(&p.colors[row], [xx, y + 0.19, -3.10], [0.13, 0.19, 0.12]);
                b.ball(&p.pale, [xx, y + 0.32, -3.10], [0.10, 0.10, 0.10]);
            }
        }
    }
}
fn plant(b: &mut Builder, p: &Palette, x: f32, z: f32) {
    b.cyl(&p.pale, [x, 0.17, z], [0.34, 0.34, 0.34]);
    b.cyl(&p.dark, [x, 0.345, z], [0.29, 0.015, 0.29]);
    for i in 0..8 {
        let a = i as f32 * 2.4;
        let end = Vec3::new(
            x + a.sin() * 0.24,
            0.55 + (i % 3) as f32 * 0.13,
            z + a.cos() * 0.23,
        );
        b.bar(&p.colors[1], Vec3::new(x, 0.3, z), end, 0.018);
        b.part(
            3,
            &p.colors[1],
            end.to_array(),
            [0.14, 0.32, 0.07],
            Quat::from_rotation_z(a.sin() * 0.7),
        );
    }
}
fn sofa(b: &mut Builder, p: &Palette, x: f32, z: f32) {
    for xx in [x - 0.77, x + 0.77] {
        for zz in [z - 0.3, z + 0.3] {
            b.cyl(&p.metal, [xx, 0.12, zz], [0.08, 0.24, 0.08]);
        }
    }
    b.soft(&p.upholstery, [x, 0.34, z], [1.95, 0.39, 0.85]);
    b.soft(&p.upholstery, [x, 0.80, z - 0.34], [1.92, 0.77, 0.23]);
    for xx in [x - 0.91, x + 0.91] {
        b.soft(&p.upholstery, [xx, 0.63, z], [0.20, 0.54, 0.91]);
    }
    for (i, xx) in [x - 0.43, x + 0.43].into_iter().enumerate() {
        b.soft(&p.pale, [xx, 0.56, z + 0.04], [0.82, 0.18, 0.73]);
        b.soft(&p.upholstery, [xx, 0.84, z - 0.18], [0.80, 0.49, 0.21]);
        b.part(
            1,
            &p.accent,
            [xx, 0.84, z + 0.03],
            [0.37, 0.37, 0.12],
            Quat::from_rotation_z(if i == 0 { 0.18 } else { -0.18 }),
        );
    }
}
fn chair(b: &mut Builder, p: &Palette, x: f32, z: f32) {
    b.cyl(&p.metal, [x, 0.30, z], [0.07, 0.48, 0.07]);
    for i in 0..5 {
        let a = i as f32 * std::f32::consts::TAU / 5.;
        let tip = Vec3::new(x + a.sin() * 0.34, 0.09, z + a.cos() * 0.34);
        b.bar(&p.dark, Vec3::new(x, 0.12, z), tip, 0.045);
        b.ball(&p.dark, tip.to_array(), [0.09, 0.09, 0.09]);
    }
    b.soft(&p.dark, [x, 0.55, z], [0.67, 0.18, 0.64]);
    b.soft(&p.accent, [x, 0.65, z], [0.54, 0.09, 0.54]);
    b.part(
        1,
        &p.dark,
        [x, 1.04, z + 0.24],
        [0.60, 0.91, 0.18],
        Quat::from_rotation_x(-0.10),
    );
    for xx in [x - 0.22, x + 0.22] {
        b.soft(&p.accent, [xx, 1.08, z + 0.13], [0.095, 0.70, 0.09]);
        b.bar(
            &p.dark,
            Vec3::new(x + (xx - x) * 1.35, 0.57, z),
            Vec3::new(x + (xx - x) * 1.35, 0.86, z),
            0.035,
        );
        b.soft(&p.dark, [x + (xx - x) * 1.35, 0.88, z], [0.11, 0.07, 0.39]);
    }
    b.soft(&p.pale, [x, 1.38, z + 0.12], [0.30, 0.14, 0.09]);
    b.soft(&p.pale, [x, 0.87, z + 0.10], [0.34, 0.17, 0.10]);
}
fn desk(b: &mut Builder, p: &Palette, x: f32) {
    b.soft(&p.wood, [x, 0.79, -2.63], [2.42, 0.12, 0.79]);
    for xx in [x - 1.02, x + 1.02] {
        b.soft(&p.dark, [xx, 0.39, -2.66], [0.11, 0.74, 0.60]);
    }
    b.soft(&p.dark, [x, 0.863, -2.51], [1.52, 0.018, 0.42]);
    for xx in [x - 0.50, x + 0.50] {
        b.soft(&p.dark, [xx, 1.27, -2.88], [0.94, 0.59, 0.075]);
        b.boxy(&p.screen, [xx, 1.28, -2.837], [0.86, 0.49, 0.012]);
        b.cyl(&p.dark, [xx, 0.94, -2.90], [0.045, 0.25, 0.045]);
        b.soft(&p.dark, [xx, 0.866, -2.83], [0.35, 0.025, 0.19]);
        // Visible layered "windows" and timeline blocks on monitor surfaces.
        for row in 0..3 {
            b.boxy(
                &p.glow,
                [
                    xx - 0.22 + row as f32 * 0.18,
                    1.14 + row as f32 * 0.075,
                    -2.825,
                ],
                [0.26, 0.025, 0.004],
            );
        }
    }
    b.soft(&p.dark, [x - 0.2, 0.887, -2.40], [0.66, 0.035, 0.22]);
    for row in 0..4 {
        for col in 0..12 {
            b.soft(
                if (row + col) % 4 == 0 {
                    &p.accent
                } else {
                    &p.pale
                },
                [
                    x - 0.49 + col as f32 * 0.052,
                    0.911,
                    -2.48 + row as f32 * 0.047,
                ],
                [0.039, 0.016, 0.032],
            );
        }
    }
    b.ball(&p.pale, [x + 0.58, 0.9, -2.39], [0.12, 0.065, 0.19]);
    // Headphone stand, ear cups and segmented curved headband.
    b.cyl(&p.metal, [x + 1.0, 1.02, -2.65], [0.025, 0.29, 0.025]);
    b.soft(&p.dark, [x + 1., 0.87, -2.65], [0.20, 0.03, 0.19]);
    for side in [-1., 1.] {
        b.soft(
            &p.dark,
            [x + 1. + side * 0.115, 1.10, -2.65],
            [0.07, 0.14, 0.12],
        );
    }
    for i in 0..8 {
        let a = i as f32 * std::f32::consts::PI / 8.;
        let c = (i + 1) as f32 * std::f32::consts::PI / 8.;
        b.bar(
            &p.accent,
            Vec3::new(x + 1. + a.cos() * 0.13, 1.12 + a.sin() * 0.13, -2.65),
            Vec3::new(x + 1. + c.cos() * 0.13, 1.12 + c.sin() * 0.13, -2.65),
            0.024,
        );
    }
    chair(b, p, x, -1.77);
}
fn curtains(b: &mut Builder, p: &Palette, x: f32) {
    b.boxy(&p.dark, [x, 2.02, -3.39], [1.65, 1.95, 0.035]);
    b.boxy(&p.screen, [x, 2.02, -3.36], [1.47, 1.80, 0.025]);
    for xx in [x - 0.75, x, x + 0.75] {
        b.boxy(&p.trim, [xx, 2.02, -3.32], [0.045, 1.88, 0.04]);
    }
    b.boxy(&p.trim, [x, 2.02, -3.32], [1.5, 0.04, 0.04]);
    b.bar(
        &p.metal,
        Vec3::new(x - 1.03, 3.12, -3.10),
        Vec3::new(x + 1.03, 3.12, -3.10),
        0.035,
    );
    for side in [-1., 1.] {
        for fold in 0..7 {
            let xx = x + side * (0.57 + fold as f32 * 0.065);
            b.soft(
                &p.upholstery,
                [xx, 1.86, -3.15 + (fold % 2) as f32 * 0.075],
                [0.115, 2.40, 0.14],
            );
        }
        b.soft(&p.metal, [x + side * 0.76, 1.35, -3.04], [0.43, 0.06, 0.07]);
    }
}
fn bed(b: &mut Builder, p: &Palette) {
    // Left-side daybed runs along Z, leaving the center stage unobstructed.
    let x = -2.38;
    let z = -1.65;
    b.soft(&p.wood, [x, 0.24, z], [1.66, 0.40, 2.54]);
    b.soft(&p.upholstery, [x, 0.94, -2.92], [1.79, 1.45, 0.18]);
    for col in 0..5 {
        for row in 0..3 {
            b.ball(
                &p.metal,
                [
                    x - 0.64 + col as f32 * 0.32,
                    0.56 + row as f32 * 0.34,
                    -2.813,
                ],
                [0.027, 0.027, 0.015],
            );
        }
    }
    b.soft(&p.pale, [x, 0.52, z], [1.59, 0.25, 2.42]);
    b.soft(&p.paper, [x, 0.66, z], [1.62, 0.06, 2.40]);
    b.soft(&p.accent, [x, 0.72, z + 0.34], [1.64, 0.15, 1.66]);
    for i in 0..9 {
        b.soft(
            &p.upholstery,
            [x - 0.74 + i as f32 * 0.185, 0.78, z + 0.60],
            [0.025, 0.012, 1.0],
        );
    }
    b.soft(&p.pale, [x, 0.80, z - 0.32], [1.63, 0.10, 0.24]);
    for xx in [x - 0.39, x + 0.39] {
        b.part(
            1,
            &p.pale,
            [xx, 0.86, -2.48],
            [0.69, 0.25, 0.48],
            Quat::from_rotation_x(0.22),
        );
        b.soft(&p.upholstery, [xx, 0.96, -2.25], [0.37, 0.32, 0.17]);
    }
    // Foot bench with pillowed upholstery and tapered-looking metal feet.
    b.soft(&p.upholstery, [x, 0.42, -0.12], [1.45, 0.21, 0.40]);
    for xx in [x - 0.56, x + 0.56] {
        b.cyl(&p.metal, [xx, 0.18, -0.12], [0.045, 0.36, 0.045]);
    }
}
fn chandelier(b: &mut Builder, p: &Palette) {
    let center = Vec3::new(-0.15, 2.70, -2.35);
    b.cyl(&p.metal, [-0.15, 3.13, -2.35], [0.035, 0.62, 0.035]);
    b.ball(&p.metal, center.to_array(), [0.20, 0.24, 0.20]);
    for i in 0..8 {
        let a = i as f32 * std::f32::consts::TAU / 8.;
        let tip = center + Vec3::new(a.cos() * 0.55, -0.03, a.sin() * 0.55);
        let elbow = center + Vec3::new(a.cos() * 0.32, -0.22, a.sin() * 0.32);
        b.bar(&p.metal, center, elbow, 0.025);
        b.bar(&p.metal, elbow, tip, 0.025);
        b.cyl(&p.metal, tip.to_array(), [0.12, 0.035, 0.12]);
        b.ball(
            &p.glow,
            (tip + Vec3::Y * 0.08).to_array(),
            [0.07, 0.15, 0.07],
        );
        for j in 0..3 {
            let pos = elbow + Vec3::new(0., -0.07 - j as f32 * 0.07, 0.);
            b.part(
                3,
                &p.glass,
                pos.to_array(),
                [0.045, 0.08, 0.045],
                Quat::from_rotation_z(0.4),
            );
        }
    }
    b.light(Color::srgb(1., 0.78, 0.64), [-0.15, 2.66, -2.35], 30000.);
}
fn fridge(b: &mut Builder, p: &Palette, x: f32) {
    b.soft(&p.dark, [x, 0.58, -2.88], [0.77, 1.14, 0.63]);
    b.boxy(&p.screen, [x, 0.59, -2.55], [0.63, 0.94, 0.018]);
    for row in 0..3 {
        let y = 0.24 + row as f32 * 0.29;
        b.boxy(&p.metal, [x, y, -2.62], [0.61, 0.023, 0.15]);
        for col in 0..4 {
            let xx = x - 0.22 + col as f32 * 0.145;
            b.cyl(
                &p.colors[(row + col) % 5],
                [xx, y + 0.11, -2.59],
                [0.09, 0.20, 0.09],
            );
            b.cyl(&p.metal, [xx, y + 0.213, -2.59], [0.08, 0.01, 0.08]);
        }
    }
    b.boxy(&p.glass, [x, 0.59, -2.51], [0.64, 0.95, 0.015]);
    b.soft(&p.metal, [x + 0.27, 0.60, -2.48], [0.035, 0.31, 0.04]);
    b.soft(&p.glow, [x, 1.13, -2.50], [0.58, 0.025, 0.025]);
}
fn hex_panels(b: &mut Builder, p: &Palette, x: f32, y: f32, rows: usize, cols: usize) {
    for row in 0..rows {
        for col in 0..cols {
            if (row == 0 && col == 0) || (row == rows - 1 && col == cols - 1) {
                continue;
            }
            let xx = x + col as f32 * 0.29 + (row % 2) as f32 * 0.145;
            let yy = y + row as f32 * 0.26;
            b.part(
                4,
                &p.glow,
                [xx, yy, -3.36],
                [0.31, 0.035, 0.31],
                Quat::from_rotation_x(std::f32::consts::FRAC_PI_2),
            );
            b.part(
                4,
                &p.pale,
                [xx, yy, -3.335],
                [0.27, 0.012, 0.27],
                Quat::from_rotation_x(std::f32::consts::FRAC_PI_2),
            );
        }
    }
}
fn artwork(
    b: &mut Builder,
    p: &Palette,
    materials: &mut Assets<StandardMaterial>,
    textures: &RoomTextures,
    x: f32,
    y: f32,
) {
    b.soft(&p.metal, [x, y, -3.35], [1.12, 0.91, 0.07]);
    b.boxy(&p.pale, [x, y, -3.30], [1.04, 0.83, 0.02]);
    if let Some(image) = &textures.wall_art {
        let m = materials.add(StandardMaterial {
            base_color_texture: Some(image.clone()),
            perceptual_roughness: 0.9,
            // Unlit keeps the painting's own colours legible in dim, tinted rooms.
            unlit: true,
            ..default()
        });
        b.part(5, &m, [x, y, -3.28], [0.94, 0.73, 1.], Quat::IDENTITY);
    } else {
        b.boxy(&p.dark, [x, y, -3.28], [0.94, 0.73, 0.015]);
        b.ball(&p.accent, [x - 0.12, y + 0.06, -3.26], [0.39, 0.40, 0.015]);
        for i in 0..4 {
            b.boxy(
                &p.colors[i],
                [
                    x + 0.1 + i as f32 * 0.075,
                    y - 0.15 + i as f32 * 0.05,
                    -3.24,
                ],
                [0.035, 0.22, 0.01],
            );
        }
    }
}
/// Four-poster canopy with sheer drapes, scalloped valance and sagging fairy lights.
fn canopy(b: &mut Builder, p: &Palette) {
    let (x0, x1, z0, z1, top) = (-3.22, -1.54, -2.84, -0.32, 2.62);
    for x in [x0, x1] {
        for z in [z0, z1] {
            b.cyl(&p.metal, [x, top / 2., z], [0.05, top, 0.05]);
            b.ball(&p.metal, [x, top + 0.04, z], [0.08, 0.08, 0.08]);
        }
        b.bar(
            &p.metal,
            Vec3::new(x, top, z0),
            Vec3::new(x, top, z1),
            0.035,
        );
    }
    for z in [z0, z1] {
        b.bar(
            &p.metal,
            Vec3::new(x0, top, z),
            Vec3::new(x1, top, z),
            0.035,
        );
    }
    for i in 0..12 {
        let z = z0 + 0.1 + i as f32 * (z1 - z0 - 0.2) / 11.;
        b.ball(&p.sheer, [x1 + 0.02, top - 0.11, z], [0.06, 0.24, 0.25]);
    }
    for i in 0..7 {
        let x = x0 + 0.1 + i as f32 * (x1 - x0 - 0.2) / 6.;
        b.ball(&p.sheer, [x, top - 0.11, z1 + 0.02], [0.27, 0.24, 0.06]);
    }
    for (x, dx) in [(x0, 1.), (x1, -1.)] {
        for (z, dz) in [(z0, 1.), (z1, -1.)] {
            for fold in 0..4 {
                let f = fold as f32;
                b.soft(
                    &p.sheer,
                    [
                        x + dx * (0.06 + f * 0.055),
                        top / 2. + 0.05,
                        z + dz * (0.04 + (fold % 2) as f32 * 0.04),
                    ],
                    [0.09, top - 0.15, 0.09],
                );
            }
            b.soft(
                &p.accent,
                [x + dx * 0.13, 1.15, z + dz * 0.06],
                [0.28, 0.05, 0.16],
            );
        }
    }
    for i in 0..15 {
        let t = i as f32 / 14.;
        let sag = (t * std::f32::consts::PI).sin() * 0.14;
        b.ball(
            &p.glow2,
            [x1 + 0.04, top - 0.05 - sag, z0 + (z1 - z0) * t],
            [0.045, 0.045, 0.045],
        );
    }
}
/// Window sill and moon, flanking sconces, and right-wall floating shelves.
fn rose_decor(b: &mut Builder, p: &Palette, x: f32) {
    b.soft(&p.trim, [x, 1.08, -3.29], [1.82, 0.07, 0.26]);
    b.boxy(&p.trim, [x, 2.97, -3.36], [1.75, 0.12, 0.06]);
    b.ball(&p.paper, [x + 0.38, 2.55, -3.344], [0.22, 0.22, 0.01]);
    for (i, xx) in [x - 0.52, x + 0.48].into_iter().enumerate() {
        b.cyl(&p.colors[i * 2], [xx, 1.21, -3.27], [0.1, 0.2, 0.1]);
        b.ball(&p.accent, [xx, 1.36, -3.27], [0.13, 0.12, 0.13]);
    }
    for xx in [-1.3, 1.5] {
        b.soft(&p.metal, [xx, 1.95, -3.45], [0.1, 0.24, 0.04]);
        b.bar(
            &p.metal,
            Vec3::new(xx, 1.92, -3.44),
            Vec3::new(xx, 2.0, -3.3),
            0.02,
        );
        b.cyl(&p.metal, [xx, 2.01, -3.3], [0.08, 0.03, 0.08]);
        b.ball(&p.glow2, [xx, 2.1, -3.3], [0.09, 0.15, 0.09]);
    }
    for y in [1.55, 1.98] {
        b.soft(&p.wood, [3.8, y, -0.7], [0.24, 0.045, 0.92]);
        for i in 0..5 {
            let z = -1.04 + i as f32 * 0.17;
            if i % 2 == 0 {
                b.cyl(
                    &p.colors[(i + 2) % 5],
                    [3.8, y + 0.1, z],
                    [0.09, 0.16, 0.09],
                );
                b.cyl(&p.metal, [3.8, y + 0.19, z], [0.07, 0.02, 0.07]);
            } else {
                b.ball(&p.pale, [3.8, y + 0.08, z], [0.11, 0.11, 0.11]);
            }
        }
    }
}
/// RGB desk edge, monitor halo, figure shelf, angular wall neon and wall slats.
fn gamer_decor(b: &mut Builder, p: &Palette, x: f32) {
    b.soft(&p.glow2, [x, 0.725, -2.235], [2.3, 0.02, 0.02]);
    for y in [0.94, 1.63] {
        b.soft(&p.glow, [x, y, -2.94], [2.0, 0.018, 0.01]);
    }
    for xx in [x - 1., x + 1.] {
        b.soft(&p.glow, [xx, 1.285, -2.94], [0.018, 0.69, 0.01]);
    }
    b.soft(&p.dark, [x, 1.96, -3.36], [1.42, 0.04, 0.22]);
    b.soft(&p.glow2, [x, 1.935, -3.26], [1.38, 0.012, 0.012]);
    for i in 0..5 {
        let xx = x - 0.56 + i as f32 * 0.28;
        if i == 2 {
            for j in 0..4 {
                b.soft(
                    &p.colors[j],
                    [xx - 0.1 + j as f32 * 0.065, 2.08, -3.36],
                    [0.05, 0.2, 0.15],
                );
            }
        } else {
            b.cyl(&p.dark, [xx, 1.995, -3.36], [0.1, 0.03, 0.1]);
            b.ball(&p.colors[i], [xx, 2.09, -3.36], [0.09, 0.16, 0.08]);
            b.ball(&p.pale, [xx, 2.21, -3.36], [0.085, 0.085, 0.085]);
        }
    }
    let neon = [
        Vec3::new(-3.7, 1.7, -3.46),
        Vec3::new(-3.2, 2.45, -3.46),
        Vec3::new(-1.35, 2.45, -3.46),
        Vec3::new(-0.95, 2.05, -3.46),
    ];
    for w in neon.windows(2) {
        b.bar(&p.glow2, w[0], w[1], 0.025);
    }
    b.bar(
        &p.glow,
        Vec3::new(-3.55, 1.7, -3.46),
        Vec3::new(-3.1, 2.32, -3.46),
        0.018,
    );
    for (i, z) in [-2.9, -2.3, -1.7, -1.1, -0.5].into_iter().enumerate() {
        let m = if i % 2 == 0 { &p.glow } else { &p.glow2 };
        b.soft(m, [-3.91, 1.8, z], [0.02, 1.6, 0.04]);
        if z < -1.5 {
            b.soft(m, [3.91, 1.8, z], [0.02, 1.6, 0.04]);
        }
    }
    b.soft(&p.glow2, [2.63, 0.03, -2.55], [1.9, 0.02, 0.78]);
}
/// Lounge set beside the hex wall: coffee table, beanbag and floor lamp.
fn lounge_decor(b: &mut Builder, p: &Palette) {
    b.soft(&p.glass, [2.7, 0.42, -1.55], [1.1, 0.04, 0.52]);
    b.soft(&p.dark, [2.7, 0.2, -1.55], [1.0, 0.03, 0.44]);
    for x in [2.22, 3.18] {
        for z in [-1.75, -1.35] {
            b.cyl(&p.metal, [x, 0.21, z], [0.04, 0.42, 0.04]);
        }
    }
    for (i, x) in [2.4, 2.62, 3.0].into_iter().enumerate() {
        b.cyl(
            &p.colors[i + 1],
            [x, 0.49, -1.5 - i as f32 * 0.05],
            [0.08, 0.1, 0.08],
        );
    }
    b.soft(&p.colors[4], [2.75, 0.22, -1.55], [0.32, 0.05, 0.24]);
    b.ball(&p.upholstery, [1.5, 0.27, -0.75], [0.86, 0.55, 0.8]);
    b.ball(&p.accent, [1.5, 0.47, -0.82], [0.5, 0.18, 0.45]);
    b.cyl(&p.dark, [3.72, 0.02, -2.95], [0.34, 0.04, 0.34]);
    b.cyl(&p.metal, [3.72, 0.8, -2.95], [0.035, 1.56, 0.035]);
    b.cyl(&p.pale, [3.72, 1.66, -2.95], [0.36, 0.26, 0.36]);
    b.ball(&p.glow2, [3.72, 1.58, -2.95], [0.14, 0.14, 0.14]);
    b.soft(&p.glow, [2.7, 0.03, -2.55], [1.9, 0.02, 0.78]);
}
pub fn build_room(
    theme: RoomTheme,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) -> RoomLayout {
    build_room_textured(theme, meshes, materials, &RoomTextures::default())
}
pub fn build_room_textured(
    theme: RoomTheme,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    textures: &RoomTextures,
) -> RoomLayout {
    let mut b = Builder::new(meshes);
    let p = palette(theme, materials);
    architecture(&mut b, &p, theme);
    match theme {
        RoomTheme::Rose => {
            bed(&mut b, &p);
            curtains(&mut b, &p, 0.1);
            shelves(&mut b, &p, 2.85);
            chandelier(&mut b, &p);
            artwork(&mut b, &p, materials, textures, -2.4, 2.48);
            // Vanity desk and upholstered stool at the right flank.
            b.soft(&p.wood, [2.72, 0.78, -1.73], [1.52, 0.10, 0.62]);
            for x in [2.12, 3.32] {
                b.cyl(&p.metal, [x, 0.39, -1.73], [0.05, 0.78, 0.05]);
            }
            b.soft(&p.pale, [2.72, 0.44, -1.15], [0.62, 0.16, 0.51]);
            for x in [2.49, 2.95] {
                b.cyl(&p.metal, [x, 0.20, -1.15], [0.045, 0.40, 0.045]);
            }
            b.ball(&p.metal, [2.72, 1.30, -1.96], [0.75, 0.91, 0.045]);
            b.ball(&p.glass, [2.72, 1.30, -1.925], [0.66, 0.81, 0.025]);
            plant(&mut b, &p, 3.4, -0.5);
            canopy(&mut b, &p);
            rose_decor(&mut b, &p, 0.1);
            b.light(Color::srgb(1., 0.72, 0.79), [2.0, 2.3, -1.2], 18000.);
            b.light(Color::srgb(0.62, 0.76, 1.), [0.1, 2.2, -2.6], 12000.);
            b.light(Color::srgb(1., 0.8, 0.55), [-2.4, 2.3, -1.6], 12000.);
        }
        RoomTheme::Violet => {
            desk(&mut b, &p, -2.25);
            shelves(&mut b, &p, 0.1);
            sofa(&mut b, &p, 2.63, -2.55);
            artwork(&mut b, &p, materials, textures, 2.65, 2.37);
            plant(&mut b, &p, 3.5, -1.45);
            // Tower PC with three glowing front fans, fluted face and glass side.
            b.soft(&p.dark, [-3.55, 0.40, -2.05], [0.44, 0.77, 0.68]);
            for y in [0.20, 0.41, 0.62] {
                b.part(
                    2,
                    &p.glow,
                    [-3.55, y, -1.70],
                    [0.16, 0.025, 0.16],
                    Quat::from_rotation_x(std::f32::consts::FRAC_PI_2),
                );
                b.ball(&p.dark, [-3.55, y, -1.68], [0.085, 0.085, 0.02]);
            }
            b.boxy(&p.glass, [-3.32, 0.41, -2.05], [0.012, 0.62, 0.52]);
            gamer_decor(&mut b, &p, -2.25);
            b.light(Color::srgb(0.64, 0.23, 1.), [-2.5, 2.5, -2.1], 25000.);
            b.light(Color::srgb(1., 0.20, 0.55), [2.9, 2.8, -2.4], 22000.);
            b.light(Color::srgb(0.85, 0.35, 1.), [0., 1.3, -2.8], 15000.);
        }
        RoomTheme::Cyan => {
            desk(&mut b, &p, -2.25);
            fridge(&mut b, &p, 0.62);
            sofa(&mut b, &p, 2.70, -2.55);
            hex_panels(&mut b, &p, 1.6, 1.45, 5, 6);
            artwork(&mut b, &p, materials, textures, -2.25, 2.52);
            plant(&mut b, &p, 3.6, -1.35);
            // Streamer's microphone arm extends above the rear desk, away from the stage.
            b.bar(
                &p.dark,
                Vec3::new(-1.30, 0.88, -2.60),
                Vec3::new(-1.25, 1.44, -2.68),
                0.035,
            );
            b.bar(
                &p.dark,
                Vec3::new(-1.25, 1.44, -2.68),
                Vec3::new(-1.85, 1.42, -2.55),
                0.035,
            );
            b.soft(&p.dark, [-1.85, 1.30, -2.55], [0.11, 0.22, 0.11]);
            b.soft(&p.accent, [-1.85, 1.33, -2.48], [0.08, 0.12, 0.025]);
            lounge_decor(&mut b, &p);
            b.light(Color::srgb(0.12, 0.85, 1.), [-2.5, 2.5, -2.0], 25000.);
            b.light(Color::srgb(0.25, 0.65, 1.), [2.8, 2.6, -2.0], 22000.);
            b.light(Color::srgb(0.75, 0.9, 1.), [3.7, 1.55, -2.9], 15000.);
        }
    }
    b.light(Color::srgb(1., 0.91, 0.85), [0., 3.1, -0.9], 45000.);
    debug_assert!(b.layout.parts.len() <= 1800);
    debug_assert!(b.layout.lights.len() <= 5);
    b.layout
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_rooms_obey_asset_and_entity_budgets() {
        for theme in [RoomTheme::Rose, RoomTheme::Violet, RoomTheme::Cyan] {
            let mut meshes = Assets::<Mesh>::default();
            let mut materials = Assets::<StandardMaterial>::default();
            let room = build_room(theme, &mut meshes, &mut materials);
            assert!(
                room.parts.len() <= 1800,
                "{theme:?}: {} parts",
                room.parts.len()
            );
            assert!(room.parts.len() > 250, "Room lost its structural detail");
            assert!(meshes.len() <= 12);
            assert!(materials.len() <= 48);
            assert!(room.lights.len() <= 5);
            for part in &room.parts {
                assert!(part.transform.translation.is_finite());
                assert!(part.transform.scale.is_finite());
                assert!(part.transform.scale.cmpgt(Vec3::ZERO).all());
                // Anything in the central foreground must remain at floor/rug level.
                let pos = part.transform.translation;
                if pos.x.abs() < 1. && pos.z > -1. {
                    assert!(pos.y < 0.06, "Foreground obstruction: {theme:?} {pos:?}");
                }
            }
        }
    }
}
