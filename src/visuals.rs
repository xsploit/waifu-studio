//! Native camera postprocessing, plus the avatar material tuning settings that
//! `shaders` applies. Material tuning is independent of camera effects.
use bevy::{
    camera::Hdr,
    core_pipeline::tonemapping::Tonemapping,
    post_process::{bloom::Bloom, effect_stack::ChromaticAberration},
    prelude::*,
    render::view::ColorGrading,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToneMap {
    #[default]
    Neutral,
    Reinhard,
    Aces,
    DisplayTransform,
}
impl ToneMap {
    pub fn label(self) -> &'static str {
        match self {
            Self::Neutral => "No tone curve",
            Self::Reinhard => "Reinhard",
            Self::Aces => "ACES fitted",
            Self::DisplayTransform => "Display transform",
        }
    }
    fn renderer(self) -> Tonemapping {
        match self {
            Self::Neutral => Tonemapping::None,
            Self::Reinhard => Tonemapping::Reinhard,
            Self::Aces => Tonemapping::AcesFitted,
            Self::DisplayTransform => Tonemapping::SomewhatBoringDisplayTransform,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VisualSettings {
    pub enabled: bool,
    pub tone_map: ToneMap,
    pub exposure: f32,
    pub saturation: f32,
    pub contrast: f32,
    pub gamma: f32,
    pub bloom: f32,
    pub chromatic_aberration: f32,
    /// Avatar material tuning; applied by `shaders` even when camera effects are off.
    pub material: MaterialTuning,
}

/// WebWaifu4's avatar material settings (`mtoon*`, `outline*`, `pbr*`) with its
/// defaults and slider ranges from `src/lib/menu/defaults.ts` / `VrmTab.tsx`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MaterialTuning {
    /// WebWaifu4 `mtoonTuning`. Off keeps every model's authored MToon values.
    pub mtoon: bool,
    /// sRGB shade color.
    pub shade_color: [f32; 3],
    pub shade_shift: f32,
    pub toony: f32,
    pub gi_equalization: f32,
    pub rim_color: [f32; 3],
    pub rim_fresnel_power: f32,
    pub rim_lift: f32,
    pub rim_lighting_mix: f32,
    pub outline_color: [f32; 3],
    pub outline_alpha: f32,
    /// WebWaifu4 `outlineThickness`; MToon width is this × 1.35, as there.
    pub outline_thickness: f32,
    /// Multipliers on each MToon material's authored matcap factor, normal-map
    /// scale and emissive factor. 1.0 keeps the model's values; independent of `mtoon`.
    pub matcap_strength: f32,
    pub normal_strength: f32,
    pub emission_strength: f32,
    /// Apply the PBR values to the Lit / PBR shading preview.
    pub pbr: bool,
    pub roughness: f32,
    pub metalness: f32,
    /// Maps to Bevy reflectance; 0.5 is the same 4% F0 as three.js specularIntensity 0.5.
    pub specular: f32,
    pub clearcoat: f32,
    pub clearcoat_roughness: f32,
}
impl Default for MaterialTuning {
    fn default() -> Self {
        let shade = 0x8a as f32 / 255.0;
        Self {
            mtoon: false,
            shade_color: [shade; 3],
            shade_shift: 0.0,
            toony: 0.9,
            gi_equalization: 0.9,
            rim_color: [1.0; 3],
            rim_fresnel_power: 5.0,
            rim_lift: 0.0,
            rim_lighting_mix: 1.0,
            outline_color: [0.0; 3],
            outline_alpha: 0.8,
            outline_thickness: 0.003,
            matcap_strength: 1.0,
            normal_strength: 1.0,
            emission_strength: 1.0,
            pbr: false,
            roughness: 0.45,
            metalness: 0.0,
            specular: 0.5,
            clearcoat: 0.3,
            clearcoat_roughness: 0.4,
        }
    }
}
impl MaterialTuning {
    pub fn outline_width(&self) -> f32 {
        self.outline_thickness * 1.35
    }
    pub fn sanitized(mut self) -> Self {
        let defaults = Self::default();
        let unit = |v: f32, fallback: f32| bounded(v, 0.0, 1.0, fallback);
        for (color, fallback) in [
            (&mut self.shade_color, defaults.shade_color),
            (&mut self.rim_color, defaults.rim_color),
            (&mut self.outline_color, defaults.outline_color),
        ] {
            for (channel, fallback) in color.iter_mut().zip(fallback) {
                *channel = unit(*channel, fallback);
            }
        }
        self.shade_shift = bounded(self.shade_shift, -1.0, 1.0, defaults.shade_shift);
        self.toony = unit(self.toony, defaults.toony);
        self.gi_equalization = unit(self.gi_equalization, defaults.gi_equalization);
        self.rim_fresnel_power = bounded(
            self.rim_fresnel_power,
            0.1,
            10.0,
            defaults.rim_fresnel_power,
        );
        self.rim_lift = unit(self.rim_lift, defaults.rim_lift);
        self.rim_lighting_mix = unit(self.rim_lighting_mix, defaults.rim_lighting_mix);
        self.outline_alpha = unit(self.outline_alpha, defaults.outline_alpha);
        self.outline_thickness = bounded(
            self.outline_thickness,
            0.0005,
            0.02,
            defaults.outline_thickness,
        );
        self.matcap_strength = bounded(self.matcap_strength, 0.0, 2.0, 1.0);
        self.normal_strength = bounded(self.normal_strength, 0.0, 2.0, 1.0);
        self.emission_strength = bounded(self.emission_strength, 0.0, 4.0, 1.0);
        self.roughness = unit(self.roughness, defaults.roughness);
        self.metalness = unit(self.metalness, defaults.metalness);
        self.specular = unit(self.specular, defaults.specular);
        self.clearcoat = unit(self.clearcoat, defaults.clearcoat);
        self.clearcoat_roughness = unit(self.clearcoat_roughness, defaults.clearcoat_roughness);
        self
    }
}

fn bounded(v: f32, lo: f32, hi: f32, fallback: f32) -> f32 {
    if v.is_finite() {
        v.clamp(lo, hi)
    } else {
        fallback
    }
}

impl Default for VisualSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            tone_map: ToneMap::Neutral,
            exposure: 0.0,
            saturation: 1.0,
            contrast: 1.0,
            gamma: 1.0,
            bloom: 0.0,
            chromatic_aberration: 0.0,
            material: MaterialTuning::default(),
        }
    }
}
impl VisualSettings {
    pub fn sanitized(mut self) -> Self {
        self.material = self.material.sanitized();
        self.exposure = bounded(self.exposure, -3.0, 3.0, 0.0);
        self.saturation = bounded(self.saturation, 0.0, 2.0, 1.0);
        self.contrast = bounded(self.contrast, 0.5, 1.8, 1.0);
        self.gamma = bounded(self.gamma, 0.5, 2.0, 1.0);
        self.bloom = bounded(self.bloom, 0.0, 0.5, 0.0);
        self.chromatic_aberration = bounded(self.chromatic_aberration, 0.0, 0.015, 0.0);
        self
    }
    /// Camera preset; material tuning is a separate choice and is kept.
    pub fn with_preset(&self, preset: VisualPreset) -> Self {
        Self {
            material: self.material.clone(),
            ..Self::preset(preset)
        }
    }
    pub fn preset(preset: VisualPreset) -> Self {
        match preset {
            VisualPreset::Neutral => Self::default(),
            // WebWaifu4 renders with ACES and toneMappingExposure 0.85 (log2 → EV),
            // and applies its PBR defaults to realistic materials.
            VisualPreset::WebWaifu4 => Self {
                enabled: true,
                tone_map: ToneMap::Aces,
                exposure: WEBWAIFU4_EXPOSURE.log2(),
                material: MaterialTuning {
                    pbr: true,
                    ..MaterialTuning::default()
                },
                ..Self::default()
            },
            VisualPreset::SoftAnime => Self {
                enabled: true,
                tone_map: ToneMap::DisplayTransform,
                exposure: 0.2,
                saturation: 1.08,
                contrast: 0.95,
                gamma: 0.95,
                bloom: 0.08,
                ..Self::default()
            },
            VisualPreset::Neon => Self {
                enabled: true,
                tone_map: ToneMap::Reinhard,
                saturation: 1.3,
                exposure: 0.4,
                contrast: 1.0,
                bloom: 0.22,
                chromatic_aberration: 0.003,
                ..Self::default()
            },
            VisualPreset::Cinematic => Self {
                enabled: true,
                tone_map: ToneMap::Aces,
                exposure: -0.15,
                saturation: 0.85,
                contrast: 1.0,
                gamma: 1.05,
                bloom: 0.05,
                ..Self::default()
            },
        }
    }
}
/// WebWaifu4 `sceneExposure` default (a linear tone-mapping multiplier).
pub const WEBWAIFU4_EXPOSURE: f32 = 0.85;
#[derive(Clone, Copy, Debug)]
pub enum VisualPreset {
    Neutral,
    WebWaifu4,
    SoftAnime,
    Neon,
    Cinematic,
}
#[derive(Resource, Default)]
pub struct VisualState {
    pub settings: VisualSettings,
    /// Root sets true for transparent desktop rendering; saved settings remain intact.
    pub suppressed: bool,
}
/// Put on the studio's main 3D camera only, never its egui/overlay camera.
#[derive(Component)]
pub struct VisualCamera;
#[derive(Component, Clone)]
struct Baseline {
    hdr: bool,
    tone: Option<Tonemapping>,
    grading: Option<ColorGrading>,
}
pub struct VisualPlugin;
impl Plugin for VisualPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<VisualState>()
            .add_systems(PostUpdate, sync);
    }
}
struct RenderPlan {
    active: bool,
    hdr: bool,
    tone: Tonemapping,
    grading: ColorGrading,
    bloom: Option<Bloom>,
    chromatic: Option<ChromaticAberration>,
}
fn plan(settings: &VisualSettings, suppressed: bool) -> RenderPlan {
    let s = settings.clone().sanitized();
    let active = s.enabled && !suppressed;
    let mut grading = ColorGrading::default();
    grading.global.exposure = s.exposure;
    for section in [
        &mut grading.shadows,
        &mut grading.midtones,
        &mut grading.highlights,
    ] {
        section.saturation = s.saturation;
        section.contrast = s.contrast;
        section.gamma = s.gamma;
    }
    RenderPlan {
        active,
        hdr: active,
        tone: s.tone_map.renderer(),
        grading,
        bloom: (active && s.bloom > 0.0).then(|| Bloom {
            intensity: s.bloom,
            ..default()
        }),
        chromatic: (active && s.chromatic_aberration > 0.0001).then(|| ChromaticAberration {
            intensity: s.chromatic_aberration,
            max_samples: 8,
            ..default()
        }),
    }
}
#[allow(clippy::type_complexity)]
fn sync(
    mut commands: Commands,
    state: Res<VisualState>,
    mut previous: Local<Option<(VisualSettings, bool)>>,
    cameras: Query<
        (
            Entity,
            Option<&Baseline>,
            Has<Hdr>,
            Option<&Tonemapping>,
            Option<&ColorGrading>,
        ),
        With<VisualCamera>,
    >,
) {
    let current = (state.settings.clone().sanitized(), state.suppressed);
    let changed = previous.as_ref() != Some(&current);
    let render = plan(&current.0, current.1);
    for (entity, baseline, hdr, tone, grading) in &cameras {
        if !changed && baseline.is_some() {
            continue;
        }
        let original = baseline.cloned().unwrap_or_else(|| Baseline {
            hdr,
            tone: tone.copied(),
            grading: grading.cloned(),
        });
        let mut camera = commands.entity(entity);
        if baseline.is_none() {
            camera.insert(original.clone());
        }
        if render.active {
            if render.hdr {
                camera.insert(Hdr);
            }
            camera.insert((render.tone, render.grading.clone()));
            if let Some(bloom) = &render.bloom {
                camera.insert(bloom.clone());
            } else {
                camera.remove::<Bloom>();
            }
            if let Some(chromatic) = &render.chromatic {
                camera.insert(chromatic.clone());
            } else {
                camera.remove::<ChromaticAberration>();
            }
        } else {
            camera.remove::<(Bloom, ChromaticAberration)>();
            if original.hdr && !state.suppressed {
                camera.insert(Hdr);
            } else {
                camera.remove::<Hdr>();
            }
            if let Some(tone) = original.tone {
                camera.insert(tone);
            } else {
                camera.remove::<Tonemapping>();
            }
            if let Some(grading) = original.grading {
                camera.insert(grading);
            } else {
                camera.remove::<ColorGrading>();
            }
        }
    }
    *previous = Some(current);
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn old_settings_default_to_previous_appearance() {
        let s: VisualSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(s, VisualSettings::default());
        assert!(!plan(&s, false).active);
    }
    #[test]
    fn bounds_and_nonfinite_inputs_are_safe() {
        let s = VisualSettings {
            exposure: f32::NAN,
            gamma: 0.0,
            bloom: 100.0,
            chromatic_aberration: f32::INFINITY,
            ..default()
        }
        .sanitized();
        assert_eq!(s.exposure, 0.0);
        assert_eq!(s.gamma, 0.5);
        assert_eq!(s.bloom, 0.5);
        assert_eq!(s.chromatic_aberration, 0.0);
    }
    #[test]
    fn settings_map_to_renderer_and_desktop_suppresses() {
        let s = VisualSettings::preset(VisualPreset::Neon);
        let p = plan(&s, false);
        assert!(p.active && p.hdr);
        assert_eq!(p.tone, Tonemapping::Reinhard);
        assert_eq!(p.grading.midtones.saturation, 1.3);
        assert_eq!(p.bloom.unwrap().intensity, 0.22);
        assert_eq!(p.chromatic.unwrap().max_samples, 8);
        let p = plan(&s, true);
        assert!(!p.active);
        assert!(p.bloom.is_none() && p.chromatic.is_none());
        assert!(s.enabled);
    }
    #[test]
    fn webwaifu4_preset_matches_its_defaults_and_presets_keep_material_choices() {
        let ww4 = VisualSettings::preset(VisualPreset::WebWaifu4);
        let p = plan(&ww4, false);
        assert_eq!(p.tone, Tonemapping::AcesFitted);
        assert!((2f32.powf(p.grading.global.exposure) - 0.85).abs() < 1e-5);
        assert!(p.bloom.is_none() && p.chromatic.is_none());
        assert!(ww4.material.pbr && !ww4.material.mtoon);
        assert_eq!(ww4.material.roughness, 0.45);
        assert!((ww4.material.outline_width() - 0.00405).abs() < 1e-7);
        let mut custom = VisualSettings::default();
        custom.material.mtoon = true;
        custom.material.toony = 0.2;
        let neon = custom.with_preset(VisualPreset::Neon);
        assert_eq!(neon.material, custom.material);
        assert_eq!(neon.bloom, 0.22);
        // Settings saved before material tuning existed load unchanged.
        let old: VisualSettings = serde_json::from_str(r#"{"enabled":true,"bloom":0.1}"#).unwrap();
        assert_eq!(old.material, MaterialTuning::default());
        let m = &old.material;
        assert_eq!(
            (m.matcap_strength, m.normal_strength, m.emission_strength),
            (1.0, 1.0, 1.0)
        );
    }
    #[test]
    fn material_tuning_is_bounded_like_webwaifu4_sliders() {
        let wild = MaterialTuning {
            shade_shift: -5.0,
            rim_fresnel_power: f32::NAN,
            outline_thickness: 1.0,
            shade_color: [2.0, f32::INFINITY, -1.0],
            roughness: 3.0,
            matcap_strength: -1.0,
            normal_strength: f32::NAN,
            emission_strength: 9.0,
            ..MaterialTuning::default()
        }
        .sanitized();
        assert_eq!(wild.matcap_strength, 0.0);
        assert_eq!(wild.normal_strength, 1.0);
        assert_eq!(wild.emission_strength, 4.0);
        assert_eq!(wild.shade_shift, -1.0);
        assert_eq!(wild.rim_fresnel_power, 5.0);
        assert_eq!(wild.outline_thickness, 0.02);
        assert_eq!(wild.shade_color, [1.0, 0x8a as f32 / 255.0, 0.0]);
        assert_eq!(wild.roughness, 1.0);
    }
    #[test]
    fn sync_restores_baseline_and_does_not_touch_unmarked_camera() {
        let mut app = App::new();
        app.add_plugins(VisualPlugin);
        let marked = app
            .world_mut()
            .spawn((VisualCamera, Tonemapping::Reinhard))
            .id();
        let unmarked = app.world_mut().spawn(Tonemapping::AcesFitted).id();
        app.world_mut().resource_mut::<VisualState>().settings =
            VisualSettings::preset(VisualPreset::Neon);
        app.update();
        assert!(app.world().entity(marked).contains::<Bloom>());
        assert!(app.world().entity(marked).contains::<Hdr>());
        assert!(!app.world().entity(unmarked).contains::<Bloom>());
        app.world_mut().resource_mut::<VisualState>().suppressed = true;
        app.update();
        assert!(!app.world().entity(marked).contains::<Bloom>());
        assert!(!app.world().entity(marked).contains::<Hdr>());
        assert_eq!(
            *app.world().entity(marked).get::<Tonemapping>().unwrap(),
            Tonemapping::Reinhard
        );
    }
}
