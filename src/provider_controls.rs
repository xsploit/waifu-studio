//! Connection management; model discovery loads when a connection is selected.
use crate::studio::Studio;
use bevy_egui::egui;
use waifu_studio::provider::{
    GatewayRoute, ProviderConfig, ReasoningEffort, RoutingSort, remap_removed_provider,
};
use waifu_studio::settings_kit as kit;

pub fn ui(ui: &mut egui::Ui, studio: &mut Studio) {
    let can_add = studio.library.providers.len() < 64;
    let mut add = None;
    ui.horizontal_wrapped(|ui| {
        ui.add_enabled_ui(can_add, |ui| {
            ui.menu_button("Add connection", |ui| {
                for preset in ProviderConfig::catalog() {
                    if ui.button(&preset.name).clicked() {
                        add = Some(preset);
                        ui.close();
                    }
                }
            });
        });
        if ui
            .add_enabled(
                can_add && !studio.library.providers.is_empty(),
                egui::Button::new("Duplicate"),
            )
            .clicked()
            && let Some(mut copy) = studio
                .library
                .providers
                .get(studio.library.provider)
                .cloned()
        {
            copy.name.push_str(" (copy)");
            // A copied configuration must explicitly choose its own session credential.
            copy.session_key.clear();
            add = Some(copy);
        }
    });
    if let Some(config) = add {
        studio.cancel_chat();
        studio.library.capture_selected_settings();
        studio.library.providers.push(config);
        studio.library.provider = studio.library.providers.len() - 1;
        studio.connected = false;
        studio.save();
    }
    if !can_add {
        ui.small("Maximum 64 connections.");
    }
    if studio.library.providers.len() > 1 {
        ui.menu_button("Remove selected connection…", |ui| {
            ui.label("Characters using this connection will use the first connection.");
            if ui.button("Remove connection").clicked() {
                let removed = studio.library.provider;
                if removed < studio.library.providers.len() {
                    studio.cancel_chat();
                    studio.library.capture_selected_settings();
                    studio.library.providers.remove(removed);
                    studio.library.provider = remap_removed_provider(removed, removed);
                    for character in &mut studio.library.characters {
                        if let Some(index) = &mut character.provider {
                            *index = remap_removed_provider(*index, removed);
                        }
                    }
                    studio.invalidate_model_catalogs();
                    studio.connected = false;
                    studio.save();
                }
                ui.close();
            }
        });
    }
    if let Some(config) = studio.library.providers.get_mut(studio.library.provider) {
        let mut route = config.route;
        kit::row(
            ui,
            "Gateway route",
            "Choose a hosted gateway or compatible endpoint.",
            |ui| {
                egui::ComboBox::from_id_salt("gateway-route")
                    .selected_text(route.map_or("Custom / compatible", GatewayRoute::label))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut route, None, "Custom / compatible");
                        for gateway in GatewayRoute::ALL {
                            ui.selectable_value(&mut route, Some(gateway), gateway.label());
                        }
                    });
            },
        );
        if route != config.route {
            config.set_route(route);
        }
        if let Some(route) = route {
            ui.small(format!(
                "WebWaifu defaults: fixed gateway URL; routed model IDs. Effective model: {}",
                config.effective_model()
            ));
            kit::note(
                ui,
                "Explicit vendor/model IDs are sent unchanged, including premium models and routing suffixes. Empty or unrouted IDs use the gateway default.",
            );
            kit::group(ui, "Upstream provider routing", |ui| {
                kit::note(
                    ui,
                    "Use exact provider slugs from this gateway's model page. Availability depends on the selected model.",
                );
                slug_list(
                    ui,
                    "Try providers in this order",
                    "provider-order",
                    &mut config.routing.order,
                );
                slug_list(
                    ui,
                    "Allow only these providers",
                    "provider-only",
                    &mut config.routing.only,
                );
                kit::note(
                    ui,
                    "An empty allow list permits all providers. Order is a preference; only is a restriction.",
                );
                kit::row(
                    ui,
                    "Provider preference",
                    "Sort eligible upstream providers.",
                    |ui| {
                        egui::ComboBox::from_id_salt("upstream-sort")
                            .selected_text(config.routing.sort.label())
                            .show_ui(ui, |ui| {
                                for sort in RoutingSort::ALL {
                                    ui.selectable_value(
                                        &mut config.routing.sort,
                                        sort,
                                        sort.label(),
                                    );
                                }
                            });
                    },
                );
                if route == GatewayRoute::OpenRouterResponses {
                    kit::toggle_row(
                        ui,
                        "Allow fallback providers",
                        "",
                        &mut config.routing.allow_fallbacks,
                    );
                    kit::note(
                        ui,
                        "For Nitro routing, append :nitro to the model ID. Nitro also permits priority endpoints and may cost more than throughput sorting.",
                    );
                    ui.hyperlink_to(
                        "Provider slugs and routing",
                        "https://openrouter.ai/docs/features/provider-routing",
                    );
                } else {
                    kit::note(
                        ui,
                        "To pin one provider without fallback, put its slug alone in the allow list. Sorting requests a preference, not a latency guarantee.",
                    );
                    ui.hyperlink_to(
                        "Provider slugs and routing",
                        "https://vercel.com/docs/ai-gateway/models-and-providers/provider-options",
                    );
                }
            });
            if route == GatewayRoute::VercelGateway {
                kit::text_row(
                    ui,
                    "OpenAI BYOK environment variable",
                    "Optional key for your own OpenAI account.",
                    &mut config.byok_openai_key_env,
                    "",
                );
            }
        }
        kit::text_row(
            ui,
            "Connection name",
            "Name shown in the connection picker.",
            &mut config.name,
            "",
        );
        kit::group(ui, "Generation budget & sampling", |ui| {
            ui.small(
                "Optional parameters depend on the model. Leave disabled to use server defaults.",
            );
            if config.uses_server_sampling() {
                kit::note(
                    ui,
                    "This GPT-5 gateway model uses server sampling defaults. Saved temperature and Top P settings are not sent.",
                );
            }
            let supported = config.supported_reasoning_efforts();
            kit::row(
                ui,
                "Reasoning effort",
                "Automatic leaves the choice to the server.",
                |ui| {
                    egui::ComboBox::from_id_salt("connection-reasoning-effort")
                        .selected_text(
                            config
                                .reasoning_effort
                                .map_or("Automatic", ReasoningEffort::label),
                        )
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut config.reasoning_effort, None, "Automatic");
                            for effort in ReasoningEffort::ALL {
                                if supported
                                    .as_ref()
                                    .is_some_and(|listed| !listed.contains(&effort))
                                {
                                    continue;
                                }
                                ui.selectable_value(
                                    &mut config.reasoning_effort,
                                    Some(effort),
                                    effort.label(),
                                );
                            }
                        });
                },
            );
            if let Some(error) = config.reasoning_effort_error() {
                ui.colored_label(egui::Color32::LIGHT_RED, error);
            }
            kit::note(
                ui,
                "Reasoning levels depend on the selected model. None explicitly disables reasoning where supported.",
            );
            let mut budget = config.max_tokens.is_some();
            if kit::toggle_row(
                ui,
                "Set output token budget (max_tokens)",
                "Limit generated output tokens.",
                &mut budget,
            )
            .changed()
            {
                config.max_tokens = budget.then_some(2048);
            }
            if let Some(tokens) = &mut config.max_tokens {
                kit::row(
                    ui,
                    "Output token budget",
                    "Maximum generated tokens.",
                    |ui| ui.add(egui::DragValue::new(tokens).range(1..=1_000_000).speed(64)),
                );
            }
            let mut nucleus = config.top_p.is_some();
            if kit::toggle_row(
                ui,
                "Set nucleus sampling (top_p)",
                "Override the server default.",
                &mut nucleus,
            )
            .changed()
            {
                config.top_p = nucleus.then_some(1.0);
            }
            if let Some(top_p) = &mut config.top_p {
                kit::slider_row(ui, "Top P", "", egui::Slider::new(top_p, 0.0..=1.0));
            }
        });
    }
    kit::note(
        ui,
        "Presets use OpenAI-compatible APIs. Anthropic and Gemini presets route through OpenRouter. Choose a model and save after editing.",
    );
}

fn slug_list(ui: &mut egui::Ui, label: &str, id: &str, slugs: &mut Vec<String>) {
    ui.push_id(id, |ui| {
        ui.label(label);
        let mut remove = None;
        for (index, slug) in slugs.iter_mut().enumerate() {
            kit::row(
                ui,
                &format!("Provider {}", index + 1),
                "Exact provider slug.",
                |ui| {
                    ui.add(
                        egui::TextEdit::singleline(slug)
                            .char_limit(128)
                            .hint_text("Exact provider slug"),
                    );
                    if ui.small_button("Remove").clicked() {
                        remove = Some(index);
                    }
                },
            );
        }
        if let Some(index) = remove {
            slugs.remove(index);
        }
        if ui
            .add_enabled(slugs.len() < 16, egui::Button::new("Add provider"))
            .clicked()
        {
            slugs.push(String::new());
        }
    });
}
