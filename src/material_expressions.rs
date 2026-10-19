//! Material expression data and reversible arithmetic. The loader attaches binds
//! to primitives; the face runtime publishes the same resolved weights used by
//! morphs, and shaders applies them only to avatar-private variants.
use bevy::{math::Affine2, prelude::*};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Reflect, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum MaterialProperty {
    Color,
    Emission,
    Shade,
    Matcap,
    Rim,
    Outline,
    Uv,
}
#[derive(Reflect, Clone, Debug)]
pub(crate) struct MaterialBind {
    pub expression: usize,
    pub property: MaterialProperty,
    pub target: [f32; 4],
    /// VRM0 vectorProperties are authoritative, including rim values omitted by
    /// the vendored importer. They seed private variants, never imported assets.
    pub authored: Option<[f32; 4]>,
    pub rgba: bool,
}
#[derive(Component, Reflect, Clone, Debug, Default)]
#[reflect(Component, Clone)]
pub(crate) struct MaterialExpressionBinds {
    pub binds: Vec<MaterialBind>,
    /// Authored base texture transform shared by Toon/Lit/Unlit variants.
    pub authored_uv: Option<[f32; 4]>,
}
#[derive(Default, Clone)]
pub(crate) struct ParsedMaterials {
    pub binds: BTreeMap<usize, MaterialExpressionBinds>,
    pub issues: BTreeSet<String>,
}
#[derive(Default)]
pub(crate) struct ResolvedMaterialWeights {
    pub manual: Vec<f32>,
    pub automatic: Vec<f32>,
    pub protected: Vec<bool>,
}
/// Rebuilt every face update. Missing roots have zero contribution, so retained
/// roots restore immediately on switch/removal even when their meshes survive.
#[derive(Resource, Default)]
pub(crate) struct MaterialExpressionFrame(pub BTreeMap<Entity, ResolvedMaterialWeights>);

fn vector<const N: usize>(value: Option<&Value>) -> Option<[f32; N]> {
    let values = value?.as_array()?;
    if values.len() != N {
        return None;
    }
    let mut result = [0.; N];
    for (out, value) in result.iter_mut().zip(values) {
        *out = value.as_f64()? as f32;
        if !out.is_finite() {
            return None;
        }
    }
    Some(result)
}
fn property(name: &str, vrm0: bool) -> Option<MaterialProperty> {
    Some(match (vrm0, name) {
        (false, "color") | (true, "_Color") => MaterialProperty::Color,
        (false, "emissionColor") | (true, "_EmissionColor") => MaterialProperty::Emission,
        (false, "shadeColor") | (true, "_ShadeColor") => MaterialProperty::Shade,
        (false, "matcapColor") => MaterialProperty::Matcap,
        (false, "rimColor") | (true, "_RimColor") => MaterialProperty::Rim,
        (false, "outlineColor") | (true, "_OutlineColor") => MaterialProperty::Outline,
        (true, "_MainTex_ST") => MaterialProperty::Uv,
        _ => return None,
    })
}
/// All shader variants share the base texture's unrotated UV0 transform.
/// Reject unsupported coordinate ownership before installing any binding.
fn authored_uv(material: &gltf::Material<'_>) -> Result<[f32; 4], &'static str> {
    let info = material.pbr_metallic_roughness().base_color_texture();
    let transform = info.as_ref().and_then(|info| info.texture_transform());
    let tex_coord = transform
        .as_ref()
        .and_then(|t| t.tex_coord())
        .unwrap_or_else(|| info.as_ref().map_or(0, |t| t.tex_coord()));
    let rotation = transform.as_ref().map_or(0., |t| t.rotation());
    if tex_coord != 0 || rotation != 0. {
        return Err("Unsupported texture transform: only unrotated base texture UV0 is supported");
    }
    let scale = transform.as_ref().map_or([1., 1.], |t| t.scale());
    let offset = transform.as_ref().map_or([0., 0.], |t| t.offset());
    let values = [scale[0], scale[1], offset[0], offset[1]];
    if values.iter().any(|v| !v.is_finite()) {
        return Err("Invalid authored texture transform");
    }
    Ok(values)
}
impl ParsedMaterials {
    pub fn count(&self) -> usize {
        self.binds.values().map(|b| b.binds.len()).sum()
    }
    pub fn parse_expression(
        &mut self,
        gltf: &gltf::Gltf,
        expression: usize,
        value: &Value,
        vrm0: bool,
        limit: usize,
    ) {
        let key = if vrm0 {
            "materialValues"
        } else {
            "materialColorBinds"
        };
        if let Some(binds) = value.get(key).and_then(Value::as_array) {
            for bind in binds.iter().take(limit) {
                if self.count() >= limit {
                    self.issues
                        .insert("Material bind limit reached; remaining binds skipped".into());
                    break;
                }
                let kind = bind
                    .get(if vrm0 { "propertyName" } else { "type" })
                    .and_then(Value::as_str)
                    .unwrap_or("<missing type>");
                let Some(property) = property(kind, vrm0) else {
                    self.issues
                        .insert(format!("Unsupported material property {kind}"));
                    continue;
                };
                let material = if vrm0 {
                    // VRM0 addresses material names. Duplicate names are ambiguous;
                    // never silently bind the wrong clothing/skin material.
                    let name = bind.get("materialName").and_then(Value::as_str);
                    let matches: Vec<_> = gltf
                        .materials()
                        .filter(|m| m.name() == name && name.is_some())
                        .collect();
                    if matches.len() == 1 {
                        matches.into_iter().next()
                    } else {
                        None
                    }
                } else {
                    bind.get("material")
                        .and_then(Value::as_u64)
                        .and_then(|i| usize::try_from(i).ok())
                        .and_then(|i| gltf.materials().nth(i))
                };
                let Some(material) = material else {
                    self.issues
                        .insert(format!("Unresolved or ambiguous material for {kind}"));
                    continue;
                };
                let Some(mut target) = vector::<4>(bind.get("targetValue")) else {
                    self.issues
                        .insert(format!("Invalid finite RGBA target for {kind}"));
                    continue;
                };
                let authored_uv = if property == MaterialProperty::Uv {
                    // Unity samples v upwards; glTF samples v downwards. For
                    // t = scale * uv + offset, conjugating by v -> 1-v gives
                    // offset_y' = 1 - offset_y - scale_y (three-vrm parity).
                    target[3] = 1. - target[3] - target[1];
                    if !target[3].is_finite() {
                        self.issues
                            .insert("Invalid converted texture transform".into());
                        continue;
                    }
                    match authored_uv(&material) {
                        Ok(value) => Some(value),
                        Err(error) => {
                            self.issues.insert(error.into());
                            continue;
                        }
                    }
                } else {
                    None
                };
                // glTF emission strength is baked into both imported material types.
                if !vrm0 && property == MaterialProperty::Emission {
                    let strength = material.emissive_strength().unwrap_or(1.);
                    for component in &mut target[..3] {
                        *component *= strength;
                    }
                    if target.iter().any(|c| !c.is_finite()) {
                        self.issues
                            .insert("Invalid emission strength/target".into());
                        continue;
                    }
                }
                let index = material.index().unwrap();
                let authored = if vrm0 && property != MaterialProperty::Uv {
                    gltf.extension_value("VRM")
                        .and_then(|v| v.get("materialProperties"))
                        .and_then(|v| v.get(index))
                        .and_then(|v| v.get("vectorProperties"))
                        .and_then(|v| vector::<4>(v.get(kind)))
                } else {
                    None
                };
                let entry = self.binds.entry(index).or_default();
                if let Some(value) = authored_uv {
                    entry.authored_uv = Some(value);
                }
                entry.binds.push(MaterialBind {
                    expression,
                    property,
                    target,
                    authored,
                    rgba: vrm0 || property == MaterialProperty::Color,
                });
                if matches!(
                    property,
                    MaterialProperty::Shade
                        | MaterialProperty::Matcap
                        | MaterialProperty::Rim
                        | MaterialProperty::Outline
                ) {
                    self.issues.insert("Shade/matcap/rim/outline expressions affect Toon only; Lit/Unlit have no matching shader terms".into());
                }
            }
            if binds.len() > limit {
                self.issues
                    .insert("Material bind limit reached; remaining binds skipped".into());
            }
        }
        if !vrm0 && let Some(binds) = value.get("textureTransformBinds").and_then(Value::as_array) {
            for bind in binds.iter().take(limit) {
                if self.count() >= limit {
                    self.issues
                        .insert("Material bind limit reached; remaining binds skipped".into());
                    break;
                }
                let material = bind
                    .get("material")
                    .and_then(Value::as_u64)
                    .and_then(|i| usize::try_from(i).ok())
                    .and_then(|i| gltf.materials().nth(i));
                let Some(material) = material else {
                    self.issues
                        .insert("Unresolved texture transform material".into());
                    continue;
                };
                let scale = if bind.get("scale").is_some() {
                    vector::<2>(bind.get("scale"))
                } else {
                    Some([1., 1.])
                };
                let offset = if bind.get("offset").is_some() {
                    vector::<2>(bind.get("offset"))
                } else {
                    Some([0., 0.])
                };
                let (Some(scale), Some(offset)) = (scale, offset) else {
                    self.issues
                        .insert("Invalid finite texture transform".into());
                    continue;
                };
                let original = match authored_uv(&material) {
                    Ok(value) => value,
                    Err(error) => {
                        self.issues.insert(error.into());
                        continue;
                    }
                };
                let index = material.index().unwrap();
                let entry = self.binds.entry(index).or_default();
                entry.authored_uv = Some(original);
                entry.binds.push(MaterialBind {
                    expression,
                    property: MaterialProperty::Uv,
                    target: [scale[0], scale[1], offset[0], offset[1]],
                    authored: None,
                    rgba: true,
                });
            }
            if binds.len() > limit {
                self.issues
                    .insert("Material bind limit reached; remaining binds skipped".into());
            }
        }
    }
}

/// Only expression-owned fields. Copies preserve the original Color variant on
/// zero weights, rather than round-tripping sRGB through linear every frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct MaterialValues {
    pub color: Color,
    pub emission: Color,
    pub shade: Color,
    pub matcap: Vec3,
    pub rim: Color,
    pub outline: Color,
    pub uv: Affine2,
}
impl MaterialValues {
    pub fn read_toon(m: &bevy_shader_mtoon::MtoonMaterial) -> Self {
        Self {
            color: m.base_color,
            emission: m.emissive_factor,
            shade: m.shade_factor,
            matcap: m.matcap_factor,
            rim: m.parametric_rim_color,
            outline: m.outline_color,
            uv: m.uv_transform,
        }
    }
    pub fn read_standard(m: &StandardMaterial) -> Self {
        Self {
            color: m.base_color,
            emission: Color::LinearRgba(m.emissive),
            uv: m.uv_transform,
            ..Self::read_toon(&bevy_shader_mtoon::MtoonMaterial::default())
        }
    }
    pub fn write_toon(&self, m: &mut bevy_shader_mtoon::MtoonMaterial) {
        m.uv_transform = self.uv;
        m.base_color = self.color;
        m.emissive_factor = self.emission;
        m.shade_factor = self.shade;
        m.matcap_factor = self.matcap;
        m.parametric_rim_color = self.rim;
        m.outline_color = self.outline;
    }
    pub fn write_standard(&self, m: &mut StandardMaterial) {
        m.base_color = self.color;
        m.emissive = self.emission.to_linear();
        m.uv_transform = self.uv;
    }
    fn get(&self, property: MaterialProperty) -> [f32; 4] {
        match property {
            MaterialProperty::Color => self.color.to_linear().to_f32_array(),
            MaterialProperty::Emission => self.emission.to_linear().to_f32_array(),
            MaterialProperty::Shade => self.shade.to_linear().to_f32_array(),
            MaterialProperty::Matcap => [self.matcap.x, self.matcap.y, self.matcap.z, 1.],
            MaterialProperty::Rim => self.rim.to_linear().to_f32_array(),
            MaterialProperty::Outline => self.outline.to_linear().to_f32_array(),
            MaterialProperty::Uv => [
                self.uv.matrix2.x_axis.x,
                self.uv.matrix2.y_axis.y,
                self.uv.translation.x,
                self.uv.translation.y,
            ],
        }
    }
    fn set(&mut self, property: MaterialProperty, value: [f32; 4]) {
        let color = || Color::LinearRgba(LinearRgba::from_f32_array(value));
        match property {
            MaterialProperty::Color => self.color = color(),
            MaterialProperty::Emission => self.emission = color(),
            MaterialProperty::Shade => self.shade = color(),
            MaterialProperty::Matcap => self.matcap = Vec3::new(value[0], value[1], value[2]),
            MaterialProperty::Rim => self.rim = color(),
            MaterialProperty::Outline => self.outline = color(),
            MaterialProperty::Uv => {
                self.uv = Affine2::from_scale_angle_translation(
                    Vec2::new(value[0], value[1]),
                    0.,
                    Vec2::new(value[2], value[3]),
                )
            }
        }
    }
    pub fn seed_authored(&mut self, binds: &MaterialExpressionBinds) {
        for bind in &binds.binds {
            if let Some(value) = bind.authored {
                self.set(bind.property, value);
            }
        }
        if let Some(value) = binds.authored_uv {
            self.set(MaterialProperty::Uv, value);
        }
    }
    /// Compose onto a freshly tuned baseline, using deltas from authored values.
    /// Automatic behavior cannot write a property shared with any protected bind.
    pub fn overlay(
        &mut self,
        original: &Self,
        binds: &MaterialExpressionBinds,
        weights: Option<&ResolvedMaterialWeights>,
        matcap_strength: f32,
        emission_strength: f32,
    ) {
        let Some(weights) = weights else {
            return;
        };
        let protected: BTreeSet<_> = binds
            .binds
            .iter()
            .filter(|b| weights.protected.get(b.expression).copied().unwrap_or(true))
            .map(|b| b.property)
            .collect();
        let mut deltas = BTreeMap::<MaterialProperty, [f64; 4]>::new();
        for bind in &binds.binds {
            let manual = weights.manual.get(bind.expression).copied().unwrap_or(0.);
            let automatic = if protected.contains(&bind.property) {
                0.
            } else {
                weights
                    .automatic
                    .get(bind.expression)
                    .copied()
                    .unwrap_or(0.)
            };
            let weight = f64::from(manual + automatic);
            if weight <= 0. || !weight.is_finite() {
                continue;
            }
            let authored = original.get(bind.property);
            let delta = deltas.entry(bind.property).or_insert([0.; 4]);
            let strength = match bind.property {
                MaterialProperty::Matcap => matcap_strength,
                MaterialProperty::Emission => emission_strength,
                _ => 1.,
            };
            for ((delta, target), authored) in delta
                .iter_mut()
                .zip(bind.target)
                .zip(authored)
                .take(if bind.rgba { 4 } else { 3 })
            {
                *delta += (f64::from(target) - f64::from(authored)) * weight * f64::from(strength);
            }
        }
        for (property, delta) in deltas {
            if delta == [0.; 4] {
                continue;
            }
            let mut value = self.get(property);
            for (value, delta) in value.iter_mut().zip(delta) {
                *value = (f64::from(*value) + delta)
                    .clamp(-f64::from(f32::MAX), f64::from(f32::MAX))
                    as f32;
            }
            self.set(property, value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn bind(expression: usize, property: MaterialProperty, target: [f32; 4]) -> MaterialBind {
        MaterialBind {
            expression,
            property,
            target,
            authored: None,
            rgba: true,
        }
    }
    #[test]
    fn vrm0_uv_target_converts_unity_coordinates_and_restores_all_variants() {
        let json = serde_json::json!({"asset":{"version":"2.0"},
            "materials":[{"name":"face", "pbrMetallicRoughness":{"baseColorTexture":{"index":0,
                "extensions":{"KHR_texture_transform":{"scale":[1.2,0.8],"offset":[0.1,0.2]}}}}}],
            "textures":[{"source":0}], "images":[{"uri":"offline.png"}]});
        let gltf =
            gltf::Gltf::from_slice_without_validation(&serde_json::to_vec(&json).unwrap()).unwrap();
        let expression = serde_json::json!({"materialValues":[{"materialName":"face",
            "propertyName":"_MainTex_ST", "targetValue":[2.0,0.5,0.25,0.1]}]});
        let mut parsed = ParsedMaterials::default();
        parsed.parse_expression(&gltf, 0, &expression, true, 32);
        assert!(parsed.issues.is_empty());
        let binds = &parsed.binds[&0];
        assert_eq!(binds.authored_uv, Some([1.2, 0.8, 0.1, 0.2]));
        let mut original = MaterialValues::read_toon(&bevy_shader_mtoon::MtoonMaterial::default());
        original.seed_authored(binds);
        for weight in [0., 0.5, 1.] {
            let mut values = original;
            values.overlay(
                &original,
                binds,
                Some(&ResolvedMaterialWeights {
                    manual: vec![weight],
                    ..default()
                }),
                1.,
                1.,
            );
            for point in [Vec2::ZERO, Vec2::ONE, Vec2::new(0.3, 0.7)] {
                let unity = Vec2::new(point.x, 1. - point.y);
                let target_unity = unity * Vec2::new(2., 0.5) + Vec2::new(0.25, 0.1);
                let target_gltf = Vec2::new(target_unity.x, 1. - target_unity.y);
                let expected = original
                    .uv
                    .transform_point2(point)
                    .lerp(target_gltf, weight);
                assert!(values.uv.transform_point2(point).distance(expected) < 1e-6);
            }
            let mut toon = bevy_shader_mtoon::MtoonMaterial::default();
            let mut standard = StandardMaterial::default();
            values.write_toon(&mut toon);
            values.write_standard(&mut standard);
            assert_eq!(toon.uv_transform, standard.uv_transform);
        }
        let mut restored = original;
        restored.overlay(&original, binds, None, 1., 1.);
        assert_eq!(restored.uv, original.uv);
        // Reject UV1/rotated bases instead of silently overwriting them.
        for transform in [
            serde_json::json!({"texCoord":1}),
            serde_json::json!({"rotation":0.5}),
        ] {
            let mut invalid = json.clone();
            invalid["materials"][0]["pbrMetallicRoughness"]["baseColorTexture"]["extensions"]["KHR_texture_transform"] =
                transform;
            let gltf =
                gltf::Gltf::from_slice_without_validation(&serde_json::to_vec(&invalid).unwrap())
                    .unwrap();
            let mut parsed = ParsedMaterials::default();
            parsed.parse_expression(&gltf, 0, &expression, true, 32);
            assert_eq!(parsed.count(), 0);
            assert!(parsed.issues.iter().any(|issue| issue.contains("UV0")));
        }
    }

    #[test]
    fn deltas_add_from_authored_values_and_never_accumulate() {
        let source = bevy_shader_mtoon::MtoonMaterial {
            base_color: Color::linear_rgba(0.5, 0.5, 0.5, 1.),
            ..default()
        };
        let original = MaterialValues::read_toon(&source);
        let binds = MaterialExpressionBinds {
            binds: vec![
                bind(0, MaterialProperty::Color, [1., 0., 0.5, 0.]),
                bind(1, MaterialProperty::Color, [0., 1., 0.5, 1.]),
            ],
            ..default()
        };
        let weights = ResolvedMaterialWeights {
            manual: vec![0.5, 0.25],
            ..default()
        };
        for _ in 0..5 {
            let mut values = original;
            values.overlay(&original, &binds, Some(&weights), 1., 1.);
            assert_eq!(
                values.color.to_linear().to_f32_array(),
                [0.625, 0.375, 0.5, 0.5]
            );
        }
        let mut restored = original;
        restored.overlay(&original, &binds, None, 1., 1.);
        assert_eq!(restored, original);
    }
    #[test]
    fn behavior_cannot_touch_a_property_shared_with_reserved_or_manual_channels() {
        let original = MaterialValues::read_toon(&bevy_shader_mtoon::MtoonMaterial::default());
        let binds = MaterialExpressionBinds {
            binds: vec![
                bind(0, MaterialProperty::Color, [0.; 4]),
                bind(1, MaterialProperty::Color, [0.5; 4]),
                bind(1, MaterialProperty::Emission, [0.8; 4]),
            ],
            ..default()
        };
        let weights = ResolvedMaterialWeights {
            manual: vec![0., 0.],
            automatic: vec![0., 0.5],
            protected: vec![true, false],
        };
        let mut values = original;
        values.overlay(&original, &binds, Some(&weights), 1., 2.);
        assert_eq!(values.color, original.color);
        assert!((values.emission.to_linear().red - 0.8).abs() < 1e-6);
    }
    #[test]
    fn zero_weights_preserve_authored_srgb_variants_and_uvs_exactly() {
        let source = StandardMaterial {
            base_color: Color::srgb(0.2, 0.3, 0.4),
            uv_transform: Affine2::from_scale_angle_translation(
                Vec2::new(2., 3.),
                0.,
                Vec2::new(0.1, 0.2),
            ),
            ..default()
        };
        let original = MaterialValues::read_standard(&source);
        let binds = MaterialExpressionBinds {
            binds: vec![bind(0, MaterialProperty::Uv, [1., 1., 0., 0.])],
            ..default()
        };
        let mut values = original;
        values.overlay(
            &original,
            &binds,
            Some(&ResolvedMaterialWeights::default()),
            1.,
            1.,
        );
        assert_eq!(values, original);
        values.overlay(
            &original,
            &binds,
            Some(&ResolvedMaterialWeights {
                manual: vec![0.5],
                ..default()
            }),
            1.,
            1.,
        );
        assert_eq!(values.uv.matrix2.x_axis.x, 1.5);
        assert_eq!(values.uv.matrix2.y_axis.y, 2.);
        assert_eq!(values.uv.translation, Vec2::new(0.05, 0.1));
    }
}
