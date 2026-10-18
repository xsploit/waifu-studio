use bevy_egui::egui;
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
    ui.horizontal(|ui| {
        ui.color_edit_button_rgb(color);
        ui.label(label);
    });
}

/// Returns true when saved settings changed. Root persists only on true.
pub fn ui(ui: &mut egui::Ui, state: &mut VisualState) -> bool {
    let before = state.settings.clone();
    ui.spacing_mut().slider_width = (ui.available_width().min(520.0) - 150.0).max(140.0);
    if state.suppressed {
        ui.label("Camera effects are suspended in transparent desktop mode to preserve transparency; saved settings are retained. Material tuning still applies.");
    }

    caption(ui, "LOOK PRESET");
    egui::ComboBox::from_id_salt("visual_preset")
        .selected_text("Apply a camera preset…")
        .width(ui.available_width().min(520.0))
        .show_ui(ui, |ui| {
            for (label, preset) in PRESETS {
                if ui.selectable_label(false, label).clicked() {
                    state.settings = state.settings.with_preset(preset);
                }
            }
        });
    ui.small(
        "Presets change camera effects; the WebWaifu4 preset also enables its PBR material values.",
    );

    caption(ui, "CAMERA EFFECTS");
    ui.checkbox(&mut state.settings.enabled, "Enable postprocessing");
    ui.add_enabled_ui(state.settings.enabled, |ui| {
        egui::ComboBox::from_id_salt("tone_mapping")
            .selected_text(state.settings.tone_map.label())
            .width(ui.available_width().min(520.0))
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
        ui.add(egui::Slider::new(&mut state.settings.exposure, -3.0..=3.0).text("Exposure (EV)"));
        ui.small(format!(
            "WebWaifu4 exposure {:.2} ≈ {:+.2} EV.",
            2f32.powf(state.settings.exposure),
            state.settings.exposure
        ));
        ui.add(egui::Slider::new(&mut state.settings.saturation, 0.0..=2.0).text("Saturation"));
        ui.add(egui::Slider::new(&mut state.settings.contrast, 0.5..=1.8).text("Contrast"));
        ui.add(egui::Slider::new(&mut state.settings.gamma, 0.5..=2.0).text("Gamma exponent"));
        ui.add(egui::Slider::new(&mut state.settings.bloom, 0.0..=0.5).text("Bloom"));
        ui.add(
            egui::Slider::new(&mut state.settings.chromatic_aberration, 0.0..=0.015)
                .fixed_decimals(4)
                .text("Chromatic aberration"),
        );
        ui.small("Gamma is an exponent: values above 1 darken midtones. Chromatic aberration uses at most 8 samples.");
        ui.checkbox(&mut state.settings.rgb_correction, "WebWaifu4 RGB color correction");
        ui.add_enabled_ui(state.settings.rgb_correction, |ui| {
            ui.small("Linear RGB power and gain before tonemapping. WebWaifu4 defaults: power 1.40 / 1.45 / 1.45, gain 1.10 each.");
            for (index, channel) in ["Red", "Green", "Blue"].into_iter().enumerate() {
                ui.add(egui::Slider::new(&mut state.settings.rgb_power[index], 0.1..=4.0).text(format!("{channel} power")));
                ui.add(egui::Slider::new(&mut state.settings.rgb_gain[index], 0.0..=4.0).text(format!("{channel} gain")));
            }
        });
    });

    let m = &mut state.settings.material;
    caption(ui, "TOON SHADING (MTOON)");
    egui::ComboBox::from_id_salt("anime_material_look")
        .selected_text("Apply an anime shading look…")
        .width(ui.available_width().min(520.0))
        .show_ui(ui, |ui| {
            for look in waifu_studio::anime_look::AnimeLook::ALL {
                if ui.selectable_label(false, look.label()).clicked() {
                    *m = look.apply(m);
                }
            }
        });
    ui.small("Use the character's Toon shading mode. Crisp cel shading uses a sharp light/shadow boundary; Original restores authored MToon values. Textures are preserved.");
    ui.checkbox(
        &mut m.mtoon,
        "Tune MToon materials (WebWaifu4 \"MToon Material\")",
    );
    ui.add_enabled_ui(m.mtoon, |ui| {
        rgb(ui, "Shade color", &mut m.shade_color);
        ui.add(egui::Slider::new(&mut m.shade_shift, -1.0..=1.0).step_by(0.05).text("Shade shift"));
        ui.add(egui::Slider::new(&mut m.toony, 0.0..=1.0).step_by(0.05).text("Toony"));
        ui.add(egui::Slider::new(&mut m.gi_equalization, 0.0..=1.0).step_by(0.05).text("GI equalize"));
        rgb(ui, "Rim color", &mut m.rim_color);
        ui.add(egui::Slider::new(&mut m.rim_lift, 0.0..=1.0).step_by(0.05).text("Rim lift"));
        ui.add(egui::Slider::new(&mut m.rim_fresnel_power, 0.1..=10.0).step_by(0.1).text("Rim power"));
        ui.add(egui::Slider::new(&mut m.rim_lighting_mix, 0.0..=1.0).step_by(0.05).text("Rim light"));
        caption(ui, "OUTLINE");
        rgb(ui, "Outline color", &mut m.outline_color);
        ui.add(
            egui::Slider::new(&mut m.outline_thickness, 0.0005..=0.02)
                .step_by(0.0005)
                .fixed_decimals(4)
                .text("Outline size"),
        );
        ui.add(egui::Slider::new(&mut m.outline_alpha, 0.0..=1.0).step_by(0.05).text("Outline alpha"));
        ui.small("Off restores each model's own values. Outline visibility is the Character outlines checkbox above; models without authored outlines keep none.");
    });

    caption(ui, "AUTHORED MTOON MAPS");
    ui.add(
        egui::Slider::new(&mut m.matcap_strength, 0.0..=2.0)
            .step_by(0.05)
            .text("Matcap strength"),
    );
    ui.add(
        egui::Slider::new(&mut m.normal_strength, 0.0..=2.0)
            .step_by(0.05)
            .text("Normal strength"),
    );
    ui.add(
        egui::Slider::new(&mut m.emission_strength, 0.0..=4.0)
            .step_by(0.05)
            .text("Emission strength"),
    );
    ui.small("Multiply each model's own matcap, normal-map and emission values in Toon mode; 1.0 keeps them. Materials without that map or value are unaffected.");

    caption(ui, "REALISTIC SHADING (PBR)");
    ui.checkbox(&mut m.pbr, "Tune the Lit / PBR shading mode");
    ui.add_enabled_ui(m.pbr, |ui| {
        ui.add(
            egui::Slider::new(&mut m.roughness, 0.0..=1.0)
                .step_by(0.05)
                .text("Roughness"),
        );
        ui.add(
            egui::Slider::new(&mut m.metalness, 0.0..=1.0)
                .step_by(0.05)
                .text("Metalness"),
        );
        ui.add(
            egui::Slider::new(&mut m.specular, 0.0..=1.0)
                .step_by(0.05)
                .text("Specular"),
        );
        ui.add(
            egui::Slider::new(&mut m.clearcoat, 0.0..=1.0)
                .step_by(0.05)
                .text("Clearcoat"),
        );
        ui.add(
            egui::Slider::new(&mut m.clearcoat_roughness, 0.0..=1.0)
                .step_by(0.05)
                .text("Clear rough"),
        );
        ui.small("Visible when Model shading is Lit / PBR.");
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
