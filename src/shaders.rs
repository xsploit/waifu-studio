//! Reversible material previews. Original VRM materials change only through opt-in
//! material tuning, which restores each material's captured values when switched off.
use crate::visuals::{MaterialTuning, VisualState};
use bevy::{asset::UntypedAssetId, prelude::*, render::render_resource::Face};
use bevy_shader_mtoon::{MtoonMaterial, OutlineMaterial, OutlineSync, VrmOutlineMode};
use std::{
    collections::HashMap,
    sync::{Arc, Weak},
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ShaderMode {
    #[default]
    Toon,
    Lit,
    Unlit,
}

#[derive(Resource)]
pub struct ShaderSettings {
    pub mode: ShaderMode,
    pub outlines: bool,
}

impl Default for ShaderSettings {
    fn default() -> Self {
        Self {
            mode: ShaderMode::Toon,
            outlines: true,
        }
    }
}

/// Attach to a model root to opt its descendants into shader switching.
#[derive(Component)]
pub struct ShaderPreview;

/// Stage geometry never participates in avatar material conversion.
#[derive(Component)]
pub struct ExcludeShaderPreview;

#[derive(Component)]
struct Variants(Arc<VariantHandles>);

struct VariantHandles {
    toon: Handle<MtoonMaterial>,
    lit: Handle<StandardMaterial>,
    unlit: Handle<StandardMaterial>,
    /// Values before any tuning, restored when tuning is switched off.
    toon_original: ToonValues,
    lit_original: PbrValues,
}

/// The MToon fields WebWaifu4's `mtoonTuning` overrides, plus the authored map
/// strengths that `MaterialTuning` scales.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ToonValues {
    shade: Color,
    shade_shift: f32,
    toony: f32,
    gi_equalization: f32,
    rim: Color,
    rim_fresnel_power: f32,
    rim_lift: f32,
    rim_lighting_mix: f32,
    outline: Color,
    outline_width: f32,
    matcap: Vec3,
    normal_scale: f32,
    emissive: Color,
}
impl ToonValues {
    fn read(m: &MtoonMaterial) -> Self {
        Self {
            shade: m.shade_factor,
            shade_shift: m.shading_shift_factor,
            toony: m.shading_toony_factor,
            gi_equalization: m.gi_equalization_factor,
            rim: m.parametric_rim_color,
            rim_fresnel_power: m.parametric_rim_fresnel_power,
            rim_lift: m.parametric_rim_lift_factor,
            rim_lighting_mix: m.rim_lighting_mix_factor,
            outline: m.outline_color,
            outline_width: m.outline_width,
            matcap: m.matcap_factor,
            normal_scale: m.normal_map_scale,
            emissive: m.emissive_factor,
        }
    }
    /// Values for `t` relative to the material's captured `original`. Map strengths
    /// scale the authored values, so 1.0 returns them bit-for-bit.
    fn target(t: &MaterialTuning, original: &Self) -> Self {
        let mut v = if t.mtoon {
            Self::tuned(t, original)
        } else {
            *original
        };
        v.matcap = original.matcap * t.matcap_strength;
        v.normal_scale = original.normal_scale * t.normal_strength;
        if t.emission_strength != 1.0 {
            let e = original.emissive.to_linear();
            let k = t.emission_strength;
            v.emissive = LinearRgba::new(e.red * k, e.green * k, e.blue * k, e.alpha).into();
        }
        v
    }
    fn tuned(t: &MaterialTuning, original: &Self) -> Self {
        let [r, g, b] = t.outline_color;
        Self {
            shade: Color::srgb_from_array(t.shade_color),
            shade_shift: t.shade_shift,
            toony: t.toony,
            gi_equalization: t.gi_equalization,
            rim: Color::srgb_from_array(t.rim_color),
            rim_fresnel_power: t.rim_fresnel_power,
            rim_lift: t.rim_lift,
            rim_lighting_mix: t.rim_lighting_mix,
            outline: Color::srgba(r, g, b, t.outline_alpha),
            outline_width: t.outline_width(),
            ..*original
        }
    }
    fn write(&self, m: &mut MtoonMaterial) {
        m.shade_factor = self.shade;
        m.shading_shift_factor = self.shade_shift;
        m.shading_toony_factor = self.toony;
        m.gi_equalization_factor = self.gi_equalization;
        m.parametric_rim_color = self.rim;
        m.parametric_rim_fresnel_power = self.rim_fresnel_power;
        m.parametric_rim_lift_factor = self.rim_lift;
        m.rim_lighting_mix_factor = self.rim_lighting_mix;
        m.outline_color = self.outline;
        m.outline_width = self.outline_width;
        m.matcap_factor = self.matcap;
        m.normal_map_scale = self.normal_scale;
        m.emissive_factor = self.emissive;
    }
}

/// The physical-material fields WebWaifu4's `pbr*` settings override.
#[derive(Clone, Copy, Debug, PartialEq)]
struct PbrValues {
    roughness: f32,
    metallic: f32,
    reflectance: f32,
    clearcoat: f32,
    clearcoat_roughness: f32,
}
impl PbrValues {
    fn read(m: &StandardMaterial) -> Self {
        Self {
            roughness: m.perceptual_roughness,
            metallic: m.metallic,
            reflectance: m.reflectance,
            clearcoat: m.clearcoat,
            clearcoat_roughness: m.clearcoat_perceptual_roughness,
        }
    }
    fn tuned(t: &MaterialTuning) -> Self {
        Self {
            roughness: t.roughness,
            metallic: t.metalness,
            reflectance: t.specular,
            clearcoat: t.clearcoat,
            clearcoat_roughness: t.clearcoat_roughness,
        }
    }
    fn write(&self, m: &mut StandardMaterial) {
        m.perceptual_roughness = self.roughness;
        m.metallic = self.metallic;
        m.reflectance = self.reflectance;
        m.clearcoat = self.clearcoat;
        m.clearcoat_perceptual_roughness = self.clearcoat_roughness;
    }
}

/// Weak entries share conversions between primitives using the same source
/// material, while allowing every texture/material handle to drop with its last
/// mesh. There is intentionally no process-lifetime strong asset cache.
#[derive(Resource, Default)]
struct VariantCache(HashMap<UntypedAssetId, Weak<VariantHandles>>);

pub struct ShaderPreviewPlugin;
impl Plugin for ShaderPreviewPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ShaderSettings>()
            .init_resource::<VariantCache>()
            .add_systems(
                Update,
                (
                    prune_variant_cache,
                    prepare_variants,
                    apply_mode,
                    apply_material_tuning,
                )
                    .chain(),
            )
            .add_systems(
                PostUpdate,
                update_outlines
                    .before(bevy::camera::visibility::VisibilitySystems::VisibilityPropagate),
            );
    }
}

fn standard_from_toon(source: &MtoonMaterial) -> StandardMaterial {
    StandardMaterial {
        base_color: source.base_color,
        base_color_texture: source.base_color_texture.clone(),
        emissive: source.emissive_factor.to_linear(),
        emissive_texture: source.emissive_texture.clone(),
        normal_map_texture: source.normal_map_texture.clone(),
        alpha_mode: source.alpha_mode,
        double_sided: source.double_sided,
        cull_mode: if source.double_sided {
            None
        } else {
            Some(Face::Back)
        },
        perceptual_roughness: 0.8,
        ..default()
    }
}

fn toon_from_standard(source: &StandardMaterial) -> MtoonMaterial {
    MtoonMaterial {
        base_color: source.base_color,
        base_color_texture: source.base_color_texture.clone(),
        emissive_factor: Color::LinearRgba(source.emissive),
        emissive_texture: source.emissive_texture.clone(),
        normal_map_texture: source.normal_map_texture.clone(),
        alpha_mode: source.alpha_mode,
        double_sided: source.double_sided || source.cull_mode.is_none(),
        shade_factor: Color::srgb(0.55, 0.58, 0.66),
        shading_toony_factor: 0.9,
        outline_mode: VrmOutlineMode::World,
        outline_width: 0.002,
        ..default()
    }
}

#[allow(clippy::type_complexity)]
fn prepare_variants(
    mut commands: Commands,
    query: Query<
        (
            Entity,
            Option<&MeshMaterial3d<MtoonMaterial>>,
            Option<&MeshMaterial3d<StandardMaterial>>,
        ),
        (
            With<Mesh3d>,
            Without<Variants>,
            Without<ExcludeShaderPreview>,
            Without<MeshMaterial3d<OutlineMaterial>>,
        ),
    >,
    parents: Query<&ChildOf>,
    roots: Query<(), With<ShaderPreview>>,
    mut toons: ResMut<Assets<MtoonMaterial>>,
    mut standards: ResMut<Assets<StandardMaterial>>,
    mut cache: ResMut<VariantCache>,
) {
    for (entity, toon, standard) in &query {
        if !parents
            .iter_ancestors(entity)
            .any(|ancestor| roots.contains(ancestor))
        {
            continue;
        }
        let key = if let Some(toon) = toon {
            toon.id().untyped()
        } else if let Some(standard) = standard {
            standard.id().untyped()
        } else {
            continue;
        };
        if let Some(shared) = cache.0.get(&key).and_then(Weak::upgrade) {
            commands
                .entity(entity)
                .insert((Variants(shared), OutlineSync));
            continue;
        }
        let (toon, lit) = if let Some(toon) = toon {
            let Some(source) = toons.get(&toon.0) else {
                continue;
            };
            (toon.0.clone(), standards.add(standard_from_toon(source)))
        } else if let Some(standard) = standard {
            let Some(source) = standards.get(&standard.0) else {
                continue;
            };
            (toons.add(toon_from_standard(source)), standard.0.clone())
        } else {
            continue;
        };
        let lit_material = standards.get(&lit).unwrap().clone();
        let lit_original = PbrValues::read(&lit_material);
        let mut unlit = lit_material;
        unlit.unlit = true;
        let unlit = standards.add(unlit);
        let Some(toon_original) = toons.get(&toon).map(ToonValues::read) else {
            continue;
        };
        let handles = Arc::new(VariantHandles {
            toon,
            lit,
            unlit,
            toon_original,
            lit_original,
        });
        cache.0.insert(key, Arc::downgrade(&handles));
        commands
            .entity(entity)
            .insert((Variants(handles), OutlineSync));
    }
}

fn prune_variant_cache(mut removed: RemovedComponents<Variants>, mut cache: ResMut<VariantCache>) {
    if removed.read().count() > 0 {
        cache.0.retain(|_, handles| handles.strong_count() != 0);
    }
}

fn apply_mode(
    mut commands: Commands,
    settings: Res<ShaderSettings>,
    all: Query<(Entity, &Variants)>,
    added: Query<(Entity, &Variants), Added<Variants>>,
) {
    let mut apply = |entity: Entity, variants: &Variants| {
        let mut entity = commands.entity(entity);
        match settings.mode {
            ShaderMode::Toon => {
                entity
                    .remove::<MeshMaterial3d<StandardMaterial>>()
                    .insert(MeshMaterial3d(variants.0.toon.clone()));
            }
            ShaderMode::Lit | ShaderMode::Unlit => {
                let material = if settings.mode == ShaderMode::Lit {
                    &variants.0.lit
                } else {
                    &variants.0.unlit
                };
                entity
                    .remove::<MeshMaterial3d<MtoonMaterial>>()
                    .insert(MeshMaterial3d(material.clone()));
            }
        }
    };
    if settings.is_changed() {
        for (entity, variants) in &all {
            apply(entity, variants);
        }
    } else {
        for (entity, variants) in &added {
            apply(entity, variants);
        }
    }
}

/// Applies WebWaifu4-style MToon and PBR tuning to every avatar material, or
/// restores the captured originals. Runs only when tuning or the avatar set changes.
fn apply_material_tuning(
    visuals: Option<Res<VisualState>>,
    all: Query<&Variants>,
    added: Query<(), Added<Variants>>,
    mut toons: ResMut<Assets<MtoonMaterial>>,
    mut standards: ResMut<Assets<StandardMaterial>>,
    mut applied: Local<Option<MaterialTuning>>,
) {
    let tuning = visuals
        .map(|v| v.settings.material.clone().sanitized())
        .unwrap_or_default();
    if applied.as_ref() == Some(&tuning) && added.is_empty() {
        return;
    }
    let mut seen = std::collections::HashSet::new();
    for variants in &all {
        let handles = &variants.0;
        if !seen.insert(Arc::as_ptr(handles)) {
            continue;
        }
        let toon = ToonValues::target(&tuning, &handles.toon_original);
        if toons
            .get(&handles.toon)
            .is_some_and(|m| ToonValues::read(m) != toon)
            && let Some(mut material) = toons.get_mut(&handles.toon)
        {
            toon.write(&mut material);
        }
        let pbr = if tuning.pbr {
            PbrValues::tuned(&tuning)
        } else {
            handles.lit_original
        };
        if standards
            .get(&handles.lit)
            .is_some_and(|m| PbrValues::read(m) != pbr)
            && let Some(mut material) = standards.get_mut(&handles.lit)
        {
            pbr.write(&mut material);
        }
    }
    *applied = Some(tuning);
}

fn update_outlines(
    settings: Res<ShaderSettings>,
    parents: Query<(), With<Variants>>,
    mut outlines: Query<(&ChildOf, &mut Visibility), With<MeshMaterial3d<OutlineMaterial>>>,
) {
    for (parent, mut visibility) in &mut outlines {
        if parents.contains(parent.parent()) {
            let desired = if settings.outlines && settings.mode == ShaderMode::Toon {
                Visibility::Inherited
            } else {
                Visibility::Hidden
            };
            if *visibility != desired {
                *visibility = desired;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn material_conversion_preserves_cutouts_and_double_sided_faces() {
        let original = MtoonMaterial {
            alpha_mode: AlphaMode::Mask(0.42),
            double_sided: true,
            base_color: Color::srgb(0.2, 0.4, 0.8),
            ..default()
        };
        let lit = standard_from_toon(&original);
        assert_eq!(lit.alpha_mode, original.alpha_mode);
        assert_eq!(lit.base_color, original.base_color);
        assert_eq!(lit.cull_mode, None);
        let toon = toon_from_standard(&lit);
        assert_eq!(toon.alpha_mode, original.alpha_mode);
        assert_eq!(toon.base_color, original.base_color);
        assert!(toon.double_sided);
    }

    #[test]
    fn material_tuning_applies_webwaifu_values_and_restores_originals() {
        let mut app = App::new();
        app.insert_resource(Assets::<MtoonMaterial>::default())
            .insert_resource(Assets::<StandardMaterial>::default())
            .init_resource::<VisualState>()
            .add_plugins(ShaderPreviewPlugin);
        let original = MtoonMaterial {
            shading_toony_factor: 0.3,
            shading_shift_factor: 0.2,
            outline_width: 0.01,
            outline_mode: VrmOutlineMode::World,
            ..default()
        };
        let handle = app
            .world_mut()
            .resource_mut::<Assets<MtoonMaterial>>()
            .add(original.clone());
        let root = app.world_mut().spawn(ShaderPreview).id();
        let mesh = app
            .world_mut()
            .spawn((
                ChildOf(root),
                Mesh3d(Handle::default()),
                MeshMaterial3d(handle.clone()),
            ))
            .id();
        app.update();
        let toon = |app: &App| {
            app.world()
                .resource::<Assets<MtoonMaterial>>()
                .get(&handle)
                .unwrap()
                .clone()
        };
        assert_eq!(toon(&app), original, "tuning is off by default");
        {
            let mut visuals = app.world_mut().resource_mut::<VisualState>();
            visuals.settings.material.mtoon = true;
            visuals.settings.material.pbr = true;
            visuals.settings.material.toony = 0.75;
        }
        app.update();
        let tuned = toon(&app);
        assert_eq!(tuned.shading_toony_factor, 0.75);
        assert_eq!(tuned.shading_shift_factor, 0.0);
        assert!((tuned.outline_width - 0.003 * 1.35).abs() < 1e-7);
        assert!((tuned.outline_color.alpha() - 0.8).abs() < 1e-6);
        assert_eq!(tuned.base_color, original.base_color);
        let lit = app.world().get::<Variants>(mesh).unwrap().0.lit.clone();
        let lit = |app: &App| {
            app.world()
                .resource::<Assets<StandardMaterial>>()
                .get(&lit)
                .unwrap()
                .clone()
        };
        assert_eq!(lit(&app).perceptual_roughness, 0.45);
        assert_eq!(lit(&app).clearcoat, 0.3);
        {
            let mut visuals = app.world_mut().resource_mut::<VisualState>();
            visuals.settings.material.mtoon = false;
            visuals.settings.material.pbr = false;
        }
        app.update();
        assert_eq!(toon(&app), original);
        assert_eq!(lit(&app).perceptual_roughness, 0.8);
        assert_eq!(lit(&app).clearcoat, 0.0);
    }

    #[test]
    fn map_strengths_scale_authored_values_and_restore_them_exactly() {
        let mut app = App::new();
        app.insert_resource(Assets::<MtoonMaterial>::default())
            .insert_resource(Assets::<StandardMaterial>::default())
            .init_resource::<VisualState>()
            .add_plugins(ShaderPreviewPlugin);
        let original = MtoonMaterial {
            matcap_factor: Vec3::new(0.5, 0.25, 1.0),
            normal_map_scale: 0.8,
            // An sRGB authored color must come back as the same variant, not a conversion.
            emissive_factor: Color::srgb(0.4, 0.2, 0.1),
            shading_toony_factor: 0.3,
            ..default()
        };
        let handle = app
            .world_mut()
            .resource_mut::<Assets<MtoonMaterial>>()
            .add(original.clone());
        let root = app.world_mut().spawn(ShaderPreview).id();
        app.world_mut().spawn((
            ChildOf(root),
            Mesh3d(Handle::default()),
            MeshMaterial3d(handle.clone()),
        ));
        app.update();
        let toon = |app: &App| {
            app.world()
                .resource::<Assets<MtoonMaterial>>()
                .get(&handle)
                .unwrap()
                .clone()
        };
        assert_eq!(toon(&app), original, "default strengths are identity");
        let set = |app: &mut App, f: &dyn Fn(&mut MaterialTuning)| {
            f(&mut app
                .world_mut()
                .resource_mut::<VisualState>()
                .settings
                .material);
            app.update();
        };
        set(&mut app, &|m| {
            m.matcap_strength = 2.0;
            m.normal_strength = 0.5;
            m.emission_strength = 3.0;
        });
        let scaled = toon(&app);
        assert_eq!(scaled.matcap_factor, Vec3::new(1.0, 0.5, 2.0));
        assert!((scaled.normal_map_scale - 0.4).abs() < 1e-6);
        let (e, o) = (
            scaled.emissive_factor.to_linear(),
            original.emissive_factor.to_linear(),
        );
        assert!((e.red - o.red * 3.0).abs() < 1e-6 && (e.blue - o.blue * 3.0).abs() < 1e-6);
        assert_eq!(
            scaled.shading_toony_factor, 0.3,
            "strengths alone keep authored shading"
        );
        // The WebWaifu4 override still scales the model's maps rather than replacing them.
        set(&mut app, &|m| m.mtoon = true);
        let both = toon(&app);
        assert_eq!(both.shading_toony_factor, 0.9);
        assert_eq!(both.matcap_factor, scaled.matcap_factor);
        assert_eq!(both.emissive_factor, scaled.emissive_factor);
        set(&mut app, &|m| *m = MaterialTuning::default());
        assert_eq!(
            toon(&app),
            original,
            "reset restores the authored material exactly"
        );
    }

    #[test]
    fn switching_modes_restores_original_toon_and_toggles_only_its_outline() {
        let mut app = App::new();
        app.insert_resource(Assets::<MtoonMaterial>::default())
            .insert_resource(Assets::<StandardMaterial>::default())
            .add_plugins(ShaderPreviewPlugin);
        let original = MtoonMaterial {
            alpha_mode: AlphaMode::Blend,
            ..default()
        };
        let handle = app
            .world_mut()
            .resource_mut::<Assets<MtoonMaterial>>()
            .add(original.clone());
        let root = app.world_mut().spawn(ShaderPreview).id();
        let mesh = app
            .world_mut()
            .spawn((
                ChildOf(root),
                Mesh3d(Handle::default()),
                MeshMaterial3d(handle.clone()),
            ))
            .id();
        let outline = app
            .world_mut()
            .spawn((
                ChildOf(mesh),
                MeshMaterial3d::<OutlineMaterial>(Handle::default()),
                Visibility::Inherited,
            ))
            .id();
        app.update();
        assert_eq!(
            app.world()
                .get::<MeshMaterial3d<MtoonMaterial>>(mesh)
                .unwrap()
                .0,
            handle
        );
        for mode in [ShaderMode::Lit, ShaderMode::Unlit] {
            app.world_mut().resource_mut::<ShaderSettings>().mode = mode;
            app.update();
            assert!(
                app.world()
                    .get::<MeshMaterial3d<MtoonMaterial>>(mesh)
                    .is_none()
            );
            let standard = &app
                .world()
                .get::<MeshMaterial3d<StandardMaterial>>(mesh)
                .unwrap()
                .0;
            let asset = app
                .world()
                .resource::<Assets<StandardMaterial>>()
                .get(standard)
                .unwrap();
            assert_eq!(asset.unlit, mode == ShaderMode::Unlit);
            assert_eq!(asset.alpha_mode, AlphaMode::Blend);
            assert_eq!(
                app.world().get::<Visibility>(outline),
                Some(&Visibility::Hidden)
            );
        }
        app.world_mut().resource_mut::<ShaderSettings>().mode = ShaderMode::Toon;
        app.update();
        assert!(
            app.world()
                .get::<MeshMaterial3d<StandardMaterial>>(mesh)
                .is_none()
        );
        assert_eq!(
            app.world()
                .get::<MeshMaterial3d<MtoonMaterial>>(mesh)
                .unwrap()
                .0,
            handle
        );
        assert_eq!(
            app.world().get::<Visibility>(outline),
            Some(&Visibility::Inherited)
        );
        assert_eq!(
            app.world().resource::<Assets<MtoonMaterial>>().get(&handle),
            Some(&original)
        );
    }

    #[test]
    fn generated_outline_children_are_not_shader_preview_candidates() {
        let mut app = App::new();
        app.insert_resource(Assets::<MtoonMaterial>::default())
            .insert_resource(Assets::<StandardMaterial>::default())
            .add_plugins(ShaderPreviewPlugin);
        let standard = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());
        let root = app.world_mut().spawn(ShaderPreview).id();
        let outline = app
            .world_mut()
            .spawn((
                ChildOf(root),
                Mesh3d(Handle::default()),
                MeshMaterial3d(standard),
                MeshMaterial3d::<OutlineMaterial>(Handle::default()),
            ))
            .id();
        app.update();
        assert!(app.world().get::<Variants>(outline).is_none());
        assert_eq!(app.world().resource::<Assets<StandardMaterial>>().len(), 1);
    }

    #[test]
    fn shared_material_variants_are_released_with_the_last_avatar_mesh() {
        let mut app = App::new();
        app.insert_resource(Assets::<MtoonMaterial>::default())
            .insert_resource(Assets::<StandardMaterial>::default())
            .add_plugins(ShaderPreviewPlugin);
        let source = app
            .world_mut()
            .resource_mut::<Assets<MtoonMaterial>>()
            .add(MtoonMaterial::default());
        let root = app.world_mut().spawn(ShaderPreview).id();
        let first = app
            .world_mut()
            .spawn((
                ChildOf(root),
                Mesh3d(Handle::default()),
                MeshMaterial3d(source.clone()),
            ))
            .id();
        let second = app
            .world_mut()
            .spawn((
                ChildOf(root),
                Mesh3d(Handle::default()),
                MeshMaterial3d(source),
            ))
            .id();
        app.update();
        let weak = {
            let first = app.world().get::<Variants>(first).unwrap();
            let second = app.world().get::<Variants>(second).unwrap();
            assert!(
                Arc::ptr_eq(&first.0, &second.0),
                "shared source must not duplicate materials"
            );
            Arc::downgrade(&first.0)
        };
        assert_eq!(app.world().resource::<Assets<StandardMaterial>>().len(), 2);
        app.world_mut().despawn(root);
        app.update();
        assert!(app.world().get_entity(first).is_err());
        assert!(app.world().get_entity(second).is_err());
        assert!(
            weak.upgrade().is_none(),
            "cache must not retain removed model handles"
        );
        assert!(app.world().resource::<VariantCache>().0.is_empty());
    }
}
