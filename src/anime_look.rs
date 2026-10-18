//! Local MToon looks. Textures stay on the model; Authored restores its values.
use crate::visuals::MaterialTuning;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnimeLook {
    Authored,
    Soft,
    Cel,
}

impl AnimeLook {
    pub const ALL: [Self; 3] = [Self::Authored, Self::Soft, Self::Cel];

    pub fn label(self) -> &'static str {
        match self {
            Self::Authored => "Model's original shading",
            Self::Soft => "Soft anime",
            Self::Cel => "Crisp cel shading",
        }
    }

    /// Leaves camera effects and PBR choices alone. The shader captures and
    /// restores each material's originals when MToon tuning is disabled.
    pub fn apply(self, current: &MaterialTuning) -> MaterialTuning {
        let mut next = current.clone();
        next.matcap_strength = 1.0;
        next.normal_strength = 1.0;
        next.emission_strength = 1.0;
        next.mtoon = self != Self::Authored;
        if self == Self::Authored {
            return next;
        }
        next.shade_shift = 0.0;
        next.gi_equalization = 0.7;
        next.rim_color = [0.08; 3];
        next.rim_fresnel_power = 5.0;
        next.rim_lift = 0.0;
        next.rim_lighting_mix = 1.0;
        next.outline_color = [0.0; 3];
        next.outline_alpha = 1.0;
        next.outline_thickness = 0.003;
        match self {
            Self::Soft => {
                next.toony = 0.65;
                next.shade_color = [0.68, 0.68, 0.72];
            }
            Self::Cel => {
                next.toony = 1.0;
                next.shade_color = [0.45, 0.48, 0.56];
            }
            Self::Authored => unreachable!(),
        }
        next
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looks_preserve_pbr_choices_and_authored_disables_all_toon_overrides() {
        let original = MaterialTuning {
            pbr: true,
            roughness: 0.23,
            matcap_strength: 0.3,
            normal_strength: 0.4,
            emission_strength: 2.0,
            ..Default::default()
        };
        let cel = AnimeLook::Cel.apply(&original);
        assert!(cel.mtoon && cel.pbr);
        assert_eq!(cel.roughness, original.roughness);
        assert_eq!(cel.toony, 1.0);
        assert!(AnimeLook::Soft.apply(&original).toony < cel.toony);
        let restored = AnimeLook::Authored.apply(&cel);
        assert!(!restored.mtoon);
        assert_eq!(
            (
                restored.matcap_strength,
                restored.normal_strength,
                restored.emission_strength
            ),
            (1.0, 1.0, 1.0)
        );
        assert_eq!(restored.roughness, original.roughness);
    }
}
