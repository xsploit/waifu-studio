use bevy_egui::egui;
use waifu_studio::settings_kit as kit;
use waifu_studio::visuals::{MaterialTuning, ToneMap, VisualPreset, VisualSettings, VisualState};

const PRESETS: [(&str, VisualPreset); 5] = [
    ("Neutral (original studio look)", VisualPreset::Neutral),
    ("WebWaifu4 (ACES, exposure 0.85)", VisualPreset::WebWaifu4),
    ("Soft anime", VisualPreset::SoftAnime),
    ("Neon", VisualPreset::Neon),
    ("Cinematic", VisualPreset::Cinematic),
];

fn caption(ui: &mut egui::Ui, text: &str) {
    ui.add_space(6.0);
    ui.label(egui::RichText::new(text).size(12.0).strong().weak());
}

fn rgb(ui: &mut egui::Ui, label: &str, color: &mut [f32; 3]) {
    kit::row(ui, label, "", |ui| {
        ui.color_edit_button_rgb(color);
    });
}

/// Returns true when saved settings changed. Root persists only on true.
pub fn ui(ui: &mut egui::Ui, state: &mut VisualState) -> bool {
    let before = state.settings.clone();
    ui.spacing_mut().slider_width = (ui.available_width().min(520.0) - 150.0).max(140.0);
    if state.suppressed {
        kit::note(
            ui,
            "Camera effects are suspended in transparent desktop mode to preserve transparency; saved settings are retained. Material tuning still applies.",
        );
    }

    kit::group(ui, "Look preset", |ui| {
        kit::row(ui, "Camera preset", "", |ui| {
            egui::ComboBox::from_id_salt("visual_preset")
                .selected_text("Apply a camera preset…")
                .width(220.0)
                .show_ui(ui, |ui| {
                    for (label, preset) in PRESETS {
                        if ui.selectable_label(false, label).clicked() {
                            state.settings = state.settings.with_preset(preset);
                        }
                    }
                });
        });
        ui.small(
        "Presets change camera effects; the WebWaifu4 preset also enables its PBR material values.",
    );
    });
    kit::group(ui, "Camera effects", |ui| {
        kit::toggle_row(ui, "Enable postprocessing", "", &mut state.settings.enabled);
        ui.add_enabled_ui(state.settings.enabled, |ui| {
        kit::row(ui, "Tone mapping", "", |ui| {
egui::ComboBox::from_id_salt("tone_mapping")
            .selected_text(state.settings.tone_map.label())
            .width(220.0)
            .show_ui(ui, |ui| {
                for tone in [
                    ToneMap::Neutral,
                    ToneMap::Reinhard,
                    ToneMap::Aces,
                    ToneMap::DisplayTransform,
                ] {
                    ui.selectable_value(&mut state.settings.tone_map, tone, tone.label());
                }
            });
});
        kit::slider_row(ui, "Exposure (EV)", "", egui::Slider::new(&mut state.settings.exposure, -3.0..=3.0));
        ui.small(format!(
            "WebWaifu4 exposure {:.2} ≈ {:+.2} EV.",
            2f32.powf(state.settings.exposure),
            state.settings.exposure
        ));
        kit::slider_row(ui, "Saturation", "", egui::Slider::new(&mut state.settings.saturation, 0.0..=2.0));
        kit::slider_row(ui, "Contrast", "", egui::Slider::new(&mut state.settings.contrast, 0.5..=1.8));
        kit::slider_row(ui, "Gamma exponent", "", egui::Slider::new(&mut state.settings.gamma, 0.5..=2.0));
        kit::slider_row(ui, "Bloom", "", egui::Slider::new(&mut state.settings.bloom, 0.0..=0.5));
        kit::slider_row(ui, "Chromatic aberration", "", egui::Slider::new(&mut state.settings.chromatic_aberration, 0.0..=0.015).fixed_decimals(4)
                ,);
        kit::note(ui, "Gamma is an exponent: values above 1 darken midtones. Chromatic aberration uses at most 8 samples.");
        kit::toggle_row(ui, "WebWaifu4 RGB color correction", "", &mut state.settings.rgb_correction);
        ui.add_enabled_ui(state.settings.rgb_correction, |ui| {
            kit::note(ui, "Linear RGB power and gain before tonemapping. WebWaifu4 defaults: power 1.40 / 1.45 / 1.45, gain 1.10 each.");
            for (index, channel) in ["Red", "Green", "Blue"].into_iter().enumerate() {
                kit::slider_row(ui, format!("{channel} power"), "", egui::Slider::new(&mut state.settings.rgb_power[index], 0.1..=4.0));
                kit::slider_row(ui, format!("{channel} gain"), "", egui::Slider::new(&mut state.settings.rgb_gain[index], 0.0..=4.0));
            }
        });
    });
    });
    let m = &mut state.settings.material;
    kit::group(ui, "Toon shading (MToon)", |ui| {
        kit::row(ui, "Anime shading look", "", |ui| {
            egui::ComboBox::from_id_salt("anime_material_look")
                .selected_text("Apply an anime shading look…")
                .width(220.0)
                .show_ui(ui, |ui| {
                    for look in waifu_studio::anime_look::AnimeLook::ALL {
                        if ui.selectable_label(false, look.label()).clicked() {
                            *m = look.apply(m);
                        }
                    }
                });
        });
        kit::note(
            ui,
            "Use the character's Toon shading mode. Crisp cel shading uses a sharp light/shadow boundary; Original restores authored MToon values. Textures are preserved.",
        );
        kit::toggle_row(
            ui,
            "Tune MToon materials (WebWaifu4 \"MToon Material\")",
            "",
            &mut m.mtoon,
        );
        ui.add_enabled_ui(m.mtoon, |ui| {
        rgb(ui, "Shade color", &mut m.shade_color);
        kit::slider_row(ui, "Shade shift", "", egui::Slider::new(&mut m.shade_shift, -1.0..=1.0).step_by(0.05));
        kit::slider_row(ui, "Toony", "", egui::Slider::new(&mut m.toony, 0.0..=1.0).step_by(0.05));
        kit::slider_row(ui, "GI equalize", "", egui::Slider::new(&mut m.gi_equalization, 0.0..=1.0).step_by(0.05));
        rgb(ui, "Rim color", &mut m.rim_color);
        kit::slider_row(ui, "Rim lift", "", egui::Slider::new(&mut m.rim_lift, 0.0..=1.0).step_by(0.05));
        kit::slider_row(ui, "Rim power", "", egui::Slider::new(&mut m.rim_fresnel_power, 0.1..=10.0).step_by(0.1));
        kit::slider_row(ui, "Rim light", "", egui::Slider::new(&mut m.rim_lighting_mix, 0.0..=1.0).step_by(0.05));
        caption(ui, "OUTLINE");
        rgb(ui, "Outline color", &mut m.outline_color);
        kit::slider_row(ui, "Outline size", "", egui::Slider::new(&mut m.outline_thickness, 0.0005..=0.02).step_by(0.0005).fixed_decimals(4)
                ,);
        kit::slider_row(ui, "Outline alpha", "", egui::Slider::new(&mut m.outline_alpha, 0.0..=1.0).step_by(0.05));
        kit::note(ui, "Off restores each model's own values. Outline visibility is the Character outlines checkbox above; models without authored outlines keep none.");
    });
    });
    kit::group(ui, "Authored MToon maps", |ui| {
        kit::slider_row(
            ui,
            "Matcap strength",
            "",
            egui::Slider::new(&mut m.matcap_strength, 0.0..=2.0).step_by(0.05),
        );
        kit::slider_row(
            ui,
            "Normal strength",
            "",
            egui::Slider::new(&mut m.normal_strength, 0.0..=2.0).step_by(0.05),
        );
        kit::slider_row(
            ui,
            "Emission strength",
            "",
            egui::Slider::new(&mut m.emission_strength, 0.0..=4.0).step_by(0.05),
        );
        kit::note(
            ui,
            "Multiply each model's own matcap, normal-map and emission values in Toon mode; 1.0 keeps them. Materials without that map or value are unaffected.",
        );
    });
    kit::group(ui, "Realistic shading (PBR)", |ui| {
        kit::toggle_row(ui, "Tune the Lit / PBR shading mode", "", &mut m.pbr);
        ui.add_enabled_ui(m.pbr, |ui| {
            kit::slider_row(
                ui,
                "Roughness",
                "",
                egui::Slider::new(&mut m.roughness, 0.0..=1.0).step_by(0.05),
            );
            kit::slider_row(
                ui,
                "Metalness",
                "",
                egui::Slider::new(&mut m.metalness, 0.0..=1.0).step_by(0.05),
            );
            kit::slider_row(
                ui,
                "Specular",
                "",
                egui::Slider::new(&mut m.specular, 0.0..=1.0).step_by(0.05),
            );
            kit::slider_row(
                ui,
                "Clearcoat",
                "",
                egui::Slider::new(&mut m.clearcoat, 0.0..=1.0).step_by(0.05),
            );
            kit::slider_row(
                ui,
                "Clear rough",
                "",
                egui::Slider::new(&mut m.clearcoat_roughness, 0.0..=1.0).step_by(0.05),
            );
            ui.small("Visible when Model shading is Lit / PBR.");
        });
    });
    ui.horizontal_wrapped(|ui| {
        if ui.button("Reset camera effects").clicked() {
            state.settings = state.settings.with_preset(VisualPreset::Neutral);
        }
        if ui.button("Reset material tuning").clicked() {
            state.settings.material = MaterialTuning::default();
        }
        if ui.button("Reset everything").clicked() {
            state.settings = VisualSettings::default();
        }
    });
    if before != state.settings {
        state.settings = state.settings.clone().sanitized();
        true
    } else {
        false
    }
}
