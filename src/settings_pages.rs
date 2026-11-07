//! Simple first screens for Voice & mic, Scene & avatar and Connections. Each keeps the
//! full original controls under an Advanced fold drawn by the caller.
use super::{Studio, decision_model_compact, model_dropdown, settings_kit as kit};
use bevy_egui::egui::{self, Color32, RichText};
use waifu_studio::{
    listen::{ListeningState, SttBackend},
    model_catalog::{CatalogKind, ModelCatalog},
    voice::{VoiceAdapter, VoiceConfig},
};

fn dropdown(ui: &mut egui::Ui, id: &str, current: usize, labels: &[&str]) -> usize {
    let mut selected = current;
    egui::ComboBox::from_id_salt(id)
        .selected_text(labels.get(current).copied().unwrap_or("—"))
        .width(ui.available_width().min(240.0))
        .truncate()
        .show_ui(ui, |ui| {
            for (index, label) in labels.iter().enumerate() {
                ui.selectable_value(&mut selected, index, *label);
            }
        });
    selected
}

fn slider(ui: &mut egui::Ui, value: &mut f32, range: std::ops::RangeInclusive<f32>) -> bool {
    ui.spacing_mut().slider_width = (ui.available_width() - 72.0).clamp(48.0, 150.0);
    ui.add(egui::Slider::new(value, range).max_decimals(2))
        .changed()
}

fn compact_models(
    ui: &mut egui::Ui,
    salt: &str,
    catalog: &ModelCatalog,
    model: &mut String,
) -> bool {
    ui.set_max_width(ui.available_width().min(260.0));
    if catalog.loading {
        ui.spinner();
    } else if catalog.models.is_empty() {
        ui.label(RichText::new("⚠").color(super::ACCENT))
            .on_hover_text(&catalog.status);
    }
    let picked = model_dropdown(ui, salt, &catalog.models, model);
    match picked {
        Some(picked) if picked != *model => {
            *model = picked;
            true
        }
        _ => false,
    }
}

fn speech_output_hint(
    output: waifu_studio::voice::VoiceOutput,
    ready: Option<usize>,
    has_character: bool,
) -> String {
    use waifu_studio::voice::VoiceOutput;
    match (output, ready) {
        (VoiceOutput::LocalOnly, _) => {
            "New speech uses local speakers. Previews stay local.".into()
        }
        (VoiceOutput::LocalAndCalls, None) => {
            "New speech uses local speakers. Call audio is unavailable in this session.".into()
        }
        (VoiceOutput::CallsOnly, None) => {
            "Call audio is unavailable in this session. Previews stay local.".into()
        }
        (VoiceOutput::CallsOnly, Some(_)) if !has_character => {
            "Select a character to use call output. Previews stay local.".into()
        }
        (VoiceOutput::LocalAndCalls, Some(_)) if !has_character => {
            "New speech uses local speakers. Select a character for call output.".into()
        }
        (VoiceOutput::CallsOnly, Some(0)) => {
            "No call output is ready. Set one up in Extensions. Applies to new speech.".into()
        }
        (VoiceOutput::CallsOnly, Some(count)) => format!(
            "{count} call output{} ready. New speech uses no local speakers. Previews stay local.",
            if count == 1 { "" } else { "s" }
        ),
        (_, Some(count)) if count > 0 => format!(
            "New speech uses local speakers and {count} ready call output{}.",
            if count == 1 { "" } else { "s" }
        ),
        _ => {
            "New speech uses local speakers. Set up call outputs in Extensions to share it.".into()
        }
    }
}
pub(super) fn voice(
    ui: &mut egui::Ui,
    studio: &mut Studio,
    speech: &mut ModelCatalog,
    transcription: &mut ModelCatalog,
    listening: &mut ListeningState,
    voices: &mut waifu_studio::voice_library::VoiceLibraryState,
    ready_calls: Option<usize>,
) {
    kit::section(ui, "Voice");
    kit::card(ui, |c| {
        let mut changed = false;
        changed |= c
            .row(
                "Read replies aloud",
                "Speaks each new reply in this character's voice.",
                |ui| kit::toggle(ui, &mut studio.library.voice.enabled),
            )
            .changed();
        let adapters = [
            VoiceAdapter::FishHosted,
            VoiceAdapter::Vercel,
            VoiceAdapter::OpenAi,
            VoiceAdapter::FishLocal,
        ];
        let current = adapters
            .iter()
            .position(|a| *a == studio.library.voice.adapter)
            .unwrap_or(0);
        let picked = c.row("Voice service", "Keys are managed in Connections.", |ui| {
            dropdown(
                ui,
                "simple_voice_adapter",
                current,
                &[
                    "Fish Audio",
                    "Vercel AI Gateway",
                    "Custom / local server",
                    "Fish Speech · local",
                ],
            )
        });
        if picked != current {
            studio.stop_voice();
            let enabled = studio.library.voice.enabled;
            let output = studio.library.voice.output;
            studio.library.voice = VoiceConfig::preset(adapters[picked]);
            studio.library.voice.enabled = enabled;
            studio.library.voice.output = output;
            studio.voice_key.clear();
            changed = true;
        }
        let has_character = studio.library.selected.is_some();
        let voice = &mut studio.library.voice;
        let outputs = [
            waifu_studio::voice::VoiceOutput::LocalAndCalls,
            waifu_studio::voice::VoiceOutput::LocalOnly,
            waifu_studio::voice::VoiceOutput::CallsOnly,
        ];
        let current = outputs
            .iter()
            .position(|mode| *mode == voice.output)
            .unwrap_or(0);
        let hint = speech_output_hint(voice.output, ready_calls, has_character);
        let picked = c.row("Speech output", &hint, |ui| {
            dropdown(
                ui,
                "speech_output",
                current,
                &["Local + calls", "Local only", "Calls only"],
            )
        });
        if picked != current {
            voice.output = outputs[picked];
            changed = true;
        }
        if has_character
            && ready_calls == Some(0)
            && voice.output != waifu_studio::voice::VoiceOutput::LocalOnly
        {
            c.row(
                "Call setup",
                "Connect a call output or review its permissions.",
                |ui| {
                    if ui.small_button("Extensions…").clicked() {
                        studio.tab = super::Tab::Extensions;
                    }
                },
            );
        }
        if voice.adapter != VoiceAdapter::FishLocal {
            speech.sync(
                &voice.endpoint,
                &voice.api_key_env,
                &studio.voice_key,
                CatalogKind::Speech,
            );
            changed |= c.row("Model", "", |ui| {
                compact_models(ui, "simple_tts_models", speech, &mut voice.model)
            });
        }
        let reference = matches!(
            voice.adapter,
            VoiceAdapter::FishHosted | VoiceAdapter::FishLocal
        );
        changed |= c.row(
            "Voice",
            if reference {
                "Voice reference ID; leave empty to use the default."
            } else {
                "Voice name or ID supported by the selected model."
            },
            |ui| {
                let value = if reference {
                    &mut voice.reference_id
                } else {
                    &mut voice.voice
                };
                ui.add(
                    egui::TextEdit::singleline(value)
                        .desired_width(ui.available_width().min(240.0))
                        .hint_text("Default"),
                )
                .changed()
            },
        );
        changed |= c.row("Speed", "", |ui| slider(ui, &mut voice.speed, 0.5..=2.0));
        let hosted = matches!(
            voice.adapter,
            VoiceAdapter::FishHosted | VoiceAdapter::Vercel
        );
        let key_loaded = !studio.voice_key.trim().is_empty()
            || loaded_environment_key(&voice.api_key_env)
            || (voice.adapter == VoiceAdapter::Vercel
                && (loaded_environment_key("AI_GATEWAY_API_KEY")
                    || loaded_environment_key("VERCEL_OIDC_TOKEN")));
        if hosted && !key_loaded {
            c.row(
                "Connection setup",
                "No key loaded. Manage your voice service in Connections.",
                |ui| {
                    if ui.small_button("Connections…").clicked() {
                        studio.tab = super::Tab::Connections;
                    }
                },
            );
        }
        c.row(
            "Try it",
            "Plays a short sample with these settings.",
            |ui| {
                if ui.button("Stop").clicked() {
                    studio.stop_voice();
                }
                if ui
                    .add_enabled(!studio.voice_busy, egui::Button::new("Preview"))
                    .clicked()
                {
                    studio.preview_voice();
                }
                if studio.voice_busy {
                    ui.spinner();
                }
            },
        );
        if changed {
            studio.save();
        }
    });

    if studio.library.voice.adapter == VoiceAdapter::FishHosted {
        kit::section(ui, "Voice library");
        kit::card(ui, |card| {
            card.block(|ui| {
                ui.collapsing("Browse Fish voices", |ui| {
                    if let Some(id) = crate::voice_library_controls::ui(
                        ui,
                        voices,
                        &studio.library.voice,
                        &studio.voice_key,
                    ) {
                        studio.library.voice.reference_id = id;
                        studio.save();
                    }
                });
            });
        });
    }
    kit::section(ui, "Microphone");
    kit::card(ui, |c| {
        let mut changed = false;
        let mut enabled = listening.config.enabled;
        if c.row(
            "Use microphone",
            "Press the mic button in chat to speak instead of typing.",
            |ui| kit::toggle(ui, &mut enabled),
        )
        .changed()
        {
            listening.set_enabled(enabled);
            changed = true;
        }
        changed |= c
            .row(
                "Send after speech",
                "Otherwise the transcript waits in the message box for you.",
                |ui| kit::toggle(ui, &mut listening.config.auto_send),
            )
            .changed();
        changed |= c.row(
            "Stop after silence",
            "Turn off to keep recording through pauses; press the microphone shortcut again to finish.",
            |ui| kit::toggle(ui, &mut listening.config.stop_on_silence),
        ).changed();
        if listening.config.stop_on_silence {
            changed |= c
                .row(
                    "Pause allowance",
                    "Wait this long before ending your turn.",
                    |ui| {
                        ui.add(
                            egui::Slider::new(
                                &mut listening.config.trailing_silence_ms,
                                200..=5000,
                            )
                            .suffix(" ms"),
                        )
                    },
                )
                .changed();
        }
        let openrouter = listening.config.backend == SttBackend::OpenAiCompatible
            && listening.config.endpoint.contains("openrouter.ai/");
        let current = match listening.config.backend {
            SttBackend::VercelGateway => 0,
            SttBackend::OpenAiCompatible if openrouter => 1,
            SttBackend::FishAsr => 2,
            SttBackend::OpenAiCompatible => 3,
        };
        let picked = c.row(
            "Transcription service",
            "Keys are managed in Connections.",
            |ui| {
                dropdown(
                    ui,
                    "simple_stt_backend",
                    current,
                    &[
                        "Vercel AI Gateway",
                        "OpenRouter",
                        "Fish Audio",
                        "Custom / local server",
                    ],
                )
            },
        );
        if picked != current {
            listening.change_service(|config| match picked {
                0 => config.preset(SttBackend::VercelGateway),
                1 => config.openrouter_preset(),
                2 => config.preset(SttBackend::FishAsr),
                _ => config.preset(SttBackend::OpenAiCompatible),
            });
            changed = true;
        }
        let config = &mut listening.config;
        transcription.sync(
            &config.endpoint,
            &config.api_key_env,
            &listening.session_key,
            CatalogKind::Transcription,
        );
        changed |= c.row("Transcription model", "", |ui| {
            compact_models(ui, "simple_stt_models", transcription, &mut config.model)
        });
        if changed {
            studio.library.listen = listening.config.clone();
            studio.save();
        }
    });
}

fn session_input(
    ui: &mut egui::Ui,
    salt: impl std::hash::Hash + std::fmt::Debug,
    key: &mut String,
) -> egui::Response {
    let id = ui.make_persistent_id(salt);
    ui.add(
        egui::TextEdit::singleline(key)
            .id(id)
            .password(true)
            .desired_width(ui.available_width().min(240.0))
            .hint_text("Use saved key"),
    )
}
/// One location for memory-only overrides; saved/environment resolution stays unchanged.
pub(super) fn session_keys(ui: &mut egui::Ui, studio: &mut Studio, listening: &mut ListeningState) {
    kit::section(ui, "Session keys");
    kit::note(
        ui,
        "These overrides last until the app closes. Leave a field empty to use saved credentials.",
    );
    kit::card(ui, |card| {
        let index = studio.library.provider;
        if let Some(provider) = studio.library.providers.get_mut(index)
            && card
                .row("Chat", &provider.name, |ui| {
                    session_input(ui, ("chat-session", index), &mut provider.session_key)
                })
                .changed()
        {
            studio.connected = false;
        }
        card.row(
            "Decision model",
            "Uses the selected decision connection.",
            |ui| {
                session_input(
                    ui,
                    "decision-session",
                    &mut studio.library.decision.session_key,
                )
            },
        );
        let voice_service = match studio.library.voice.adapter {
            VoiceAdapter::FishHosted => "Fish Audio",
            VoiceAdapter::Vercel => "Vercel AI Gateway",
            VoiceAdapter::FishLocal => "Fish Speech · local",
            VoiceAdapter::OpenAi => "Custom / local server",
        };
        card.row("Voice", voice_service, |ui| {
            session_input(ui, "voice-session", &mut studio.voice_key)
        });
        let transcription_service = match listening.config.backend {
            SttBackend::VercelGateway => "Vercel AI Gateway",
            SttBackend::FishAsr => "Fish Audio",
            SttBackend::OpenAiCompatible
                if listening.config.endpoint.contains("openrouter.ai/") =>
            {
                "OpenRouter"
            }
            SttBackend::OpenAiCompatible => "Custom / local server",
        };
        card.row("Microphone", transcription_service, |ui| {
            session_input(ui, "microphone-session", &mut listening.session_key)
        });
    });
}

pub(super) fn connections(ui: &mut egui::Ui, studio: &mut Studio, listening: &ListeningState) {
    kit::section(ui, "Chat");
    kit::card(ui, |c| {
        let previous = studio.library.provider;
        let names: Vec<&str> = studio
            .library
            .providers
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        let picked = c.row("Provider", "Where chat replies come from.", |ui| {
            dropdown(ui, "simple_connection", previous, &names)
        });
        if picked != previous {
            studio.library.provider = picked;
            studio.connected = false;
            studio.save();
        }
        let index = studio.library.provider;
        studio.ensure_models();
        let models = studio.model_lists.get(&index).cloned().unwrap_or_default();
        let loading = studio.models_status().0;
        let current = studio
            .library
            .providers
            .get(index)
            .map(|p| p.model.clone())
            .unwrap_or_default();
        let picked = c.row("Model", "", |ui| {
            ui.set_max_width(ui.available_width().min(260.0));
            if loading {
                ui.spinner();
            }
            model_dropdown(ui, "simple_connection_models", &models, &current)
        });
        if let Some(model) = picked
            && let Some(provider) = studio.library.providers.get_mut(index)
            && provider.model != model
        {
            provider.reasoning_effort = None;
            provider.model = model;
            studio.connected = false;
            studio.save();
        }
        if let Some(provider) = studio.library.providers.get_mut(index)
            && c.row(
                "Creativity",
                "Higher is more playful, lower is more focused.",
                |ui| slider(ui, &mut provider.temperature, 0.0..=2.0),
            )
        {
            studio.save();
        }
    });

    kit::section(ui, "Decision model");
    kit::card(ui, |c| {
        let connection = studio.library.decision.clone();
        let mut model = connection.model.clone();
        c.row(
            "Model",
            "Chooses memory suggestions, reactions and replies.",
            |ui| {
                decision_model_compact(
                    ui,
                    "connections_decision_model",
                    &mut studio.decision_models,
                    &connection,
                    &mut model,
                )
            },
        );
        if model != studio.library.decision.model {
            studio.library.decision.model = model;
            studio.save();
        }
    });

    kit::section(ui, "Keys");
    let configured =
        |session: &str, env: &str| !session.trim().is_empty() || loaded_environment_key(env);
    let gateway =
        loaded_environment_key("AI_GATEWAY_API_KEY") || loaded_environment_key("VERCEL_OIDC_TOKEN");
    let decision = &studio.library.decision;
    let voice = &studio.library.voice;
    let keys = [
        (
            "Chat",
            studio
                .library
                .providers
                .get(studio.library.provider)
                .is_some_and(|p| p.has_key()),
        ),
        (
            "Decision model",
            configured(&decision.session_key, &decision.key_env)
                || (decision.key_env.trim() == "AI_GATEWAY_API_KEY" && gateway),
        ),
        (
            "Voice",
            configured(&studio.voice_key, &voice.api_key_env)
                || (voice.adapter == VoiceAdapter::Vercel && gateway),
        ),
        (
            "Microphone",
            configured(&listening.session_key, &listening.config.api_key_env)
                || (listening.config.backend == SttBackend::VercelGateway
                    && listening.config.api_key_env.trim() == "AI_GATEWAY_API_KEY"
                    && gateway),
        ),
    ];
    kit::card(ui, |c| {
        for (label, loaded) in keys {
            c.row(label, "", |ui| {
                ui.label(RichText::new(if loaded { "Loaded" } else { "Not loaded" })
                    .color(if loaded { Color32::from_rgb(120, 190, 140) } else { super::MUTED }))
                    .on_hover_text("Reports saved or session credentials, not a successful connection. Local servers may not need a key.");
            });
        }
        c.row(
            "Session overrides",
            "Saved gateway keys are reused automatically.",
            |ui| {
                if ui.small_button("Manage…").clicked() {
                    ui.ctx().data_mut(|data| {
                        data.insert_temp(egui::Id::new(("open-advanced", "connections")), true)
                    });
                }
            },
        );
    });
}
fn loaded_environment_key(name: &str) -> bool {
    !name.trim().is_empty()
        && std::env::var(name.trim()).is_ok_and(|value| !value.trim().is_empty())
}

pub(super) fn group_chat(
    ui: &mut egui::Ui,
    studio: &mut Studio,
    chat: &mut waifu_studio::scene_chat::SceneChat,
    roster: &mut waifu_studio::scene_roster::SceneRoster,
) {
    let library = &studio.library;
    let busy = chat.is_busy();
    kit::section(ui, "In the scene");
    ui.add_enabled_ui(!busy, |ui| {
        kit::card(ui, |c| {
            let mut remove = None;
            for character in &library.characters {
                let focused = library.selected == Some(character.id);
                if !focused && !roster.contains(character.id) {
                    continue;
                }
                let provider = if focused {
                    library.provider
                } else {
                    character.provider.unwrap_or(library.provider)
                };
                let model = library
                    .providers
                    .get(provider)
                    .map(|p| p.effective_model())
                    .unwrap_or_else(|| "No connection".into());
                let hint = if focused {
                    format!("Main companion · {model}")
                } else {
                    model
                };
                let mut replies = chat.recipients.contains(&character.id);
                let (toggled, removed) = c.row(&character.name, &hint, |ui| {
                    let toggled = kit::toggle(ui, &mut replies).changed();
                    let removed = !focused && ui.small_button("Remove").clicked();
                    (toggled, removed)
                });
                if toggled {
                    if replies {
                        chat.recipients.push(character.id);
                    } else {
                        chat.recipients.retain(|id| *id != character.id);
                    }
                }
                if removed {
                    remove = Some(character.id);
                }
            }
            if let Some(id) = remove {
                chat.recipients.retain(|r| *r != id);
                roster.remove(id);
            }
            if roster.slots.len() < 3 {
                c.row(
                    "Add a companion",
                    "Up to four characters in one scene.",
                    |ui| {
                        egui::ComboBox::from_id_salt("simple_add_companion")
                            .selected_text("Choose…")
                            .width(ui.available_width().min(240.0))
                            .truncate()
                            .show_ui(ui, |ui| {
                                for character in &library.characters {
                                    if Some(character.id) != library.selected
                                        && !roster.contains(character.id)
                                        && ui.selectable_label(false, &character.name).clicked()
                                    {
                                        let _ = roster.add(
                                            character.id,
                                            character.name.clone(),
                                            character.model.clone(),
                                        );
                                    }
                                }
                            });
                    },
                );
            }
        });
    });
    kit::section(ui, "Message");
    kit::card(ui, |c| {
        c.block(|ui| {
            ui.add_enabled(
                !busy,
                egui::TextEdit::multiline(&mut chat.draft)
                    .desired_rows(3)
                    .desired_width(f32::INFINITY)
                    .hint_text("Say something to the scene… (@name works too)"),
            );
            ui.horizontal_wrapped(|ui| {
                if busy {
                    ui.spinner();
                    if ui.button("Cancel").clicked() {
                        chat.cancel();
                    }
                } else {
                    let send = ui.add(
                        egui::Button::new(RichText::new("Send").color(Color32::WHITE))
                            .fill(Color32::from_rgb(184, 88, 96)),
                    );
                    if send.clicked()
                        && let Err(error) = chat.submit(library, roster)
                    {
                        chat.status = error;
                    }
                    if chat.recipients.is_empty()
                        && ui
                            .button("Pick who replies")
                            .on_hover_text("The decision model chooses from the scene.")
                            .clicked()
                        && let Err(error) = chat.route_draft(&library.decision, library, roster)
                    {
                        chat.status = error;
                    }
                }
            });
            if !chat.partial.is_empty() {
                ui.label(&chat.partial);
            }
            if !chat.status.is_empty() {
                ui.label(RichText::new(&chat.status).size(12.0).weak());
            }
        });
    });
}

pub(super) fn tools(ui: &mut egui::Ui, mcp: &mut waifu_studio::mcp::McpState, studio: &mut Studio) {
    kit::section(ui, "Companion agent loop");
    let mut changed = false;
    if let Some(character) = studio.library.selected_character_mut() {
        let settings = &mut character.delegation.tools;
        kit::card(ui, |c| {
            changed |= c
                .row(
                    "Use tools automatically",
                    "Search, inspect results, delegate and continue during your conversation.",
                    |ui| kit::toggle(ui, &mut settings.enabled),
                )
                .changed();
            changed |= c.row("Tavily web search", if waifu_studio::companion_tools::tavily_ready() { "Private search key is available." } else { "Set TAVILY_API_KEY in your private credentials.env or environment, then restart." },
                |ui| kit::toggle(ui, &mut settings.tavily)).changed();
            changed |= c
                .row(
                    "Delegate to Codex",
                    "Read-only tasks in the local workspace configured under Agent tasks.",
                    |ui| kit::toggle(ui, &mut settings.codex),
                )
                .changed();
            changed |= c
                .row(
                    "Delegate to Claude",
                    "Read-only tasks using the configured Claude model and local login.",
                    |ui| kit::toggle(ui, &mut settings.claude),
                )
                .changed();
            changed |= c
                .row(
                    "Calls per message",
                    "The companion gives a final answer when this budget is reached.",
                    |ui| ui.add(egui::Slider::new(&mut settings.max_calls, 1..=16)),
                )
                .changed();
            c.block(|ui| { ui.label("Off by default. Tool use adds service and model round trips. Your selected model must support OpenAI-compatible function calls; unsupported models report an error. Browser capabilities can be supplied by an MCP browser server. Load its tools below and enable the specific tools Hikari may call. Progress display and narration are configured below."); });
        });
    }
    kit::section(ui, "Progress updates");
    kit::card(ui, |c| {
        let mut changed = false;
        if let Some(character) = studio.library.selected_character_mut() {
            changed |= c
                .row(
                    "Show intermediate text",
                    "Display progress between companion tool calls and delegated tasks.",
                    |ui| kit::toggle(ui, &mut character.delegation.show_intermediate_text),
                )
                .changed();
            changed |= c
                .row(
                    "Speak intermediate text",
                    "Read progress aloud; final answers still follow your voice setting.",
                    |ui| kit::toggle(ui, &mut character.delegation.speak_intermediate_text),
                )
                .changed();
        }
        if changed {
            studio.save();
        }
    });
    let busy = mcp.is_busy();
    kit::section(ui, "Tool server");
    kit::card(ui, |c| {
        c.row(
            "Server address",
            "An MCP server reachable over HTTP.",
            |ui| {
                ui.add_enabled_ui(!busy, |ui| {
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut mcp.endpoint)
                                .desired_width(ui.available_width().min(240.0)),
                        )
                        .changed()
                    {
                        mcp.disarm();
                    }
                });
            },
        );
        let status = mcp.status.clone();
        c.row("Tools", &status, |ui| {
            if busy {
                if ui.button("Cancel").clicked() {
                    mcp.cancel();
                }
                ui.spinner();
            } else if ui.button("Load tools").clicked() {
                mcp.list_tools();
            }
        });
        c.block(|ui| {
            kit::advanced(ui, "tools_connection", |ui| {
                kit::row(
                    ui,
                    "Access token",
                    "Optional. Kept for this session only.",
                    |ui| {
                        ui.add_enabled(
                            !busy,
                            egui::TextEdit::singleline(&mut mcp.session_token)
                                .password(true)
                                .desired_width(ui.available_width().min(240.0)),
                        );
                    },
                );
            });
        });
    });
    if !mcp.tools.is_empty() {
        kit::section(ui, "Tools the companion may call");
        if let Some(character) = studio.library.selected_character_mut() {
            let settings = &mut character.delegation.tools;
            let endpoint = mcp.endpoint.trim();
            for tool in mcp.tools.iter().take(32) {
                let mut enabled =
                    settings.mcp_endpoint == endpoint && settings.mcp_tools.contains(&tool.name);
                if ui.checkbox(&mut enabled, &tool.name).on_hover_text("Allow automatic calls to this tool during a chat request. Only enable tools whose effects you want the companion to perform.").changed() {
                    if settings.mcp_endpoint != endpoint {
                        settings.mcp_endpoint = endpoint.into();
                        settings.mcp_tools.clear();
                    }
                    settings.mcp_tools.retain(|name| name != &tool.name);
                    if enabled { settings.mcp_tools.push(tool.name.clone()); }
                    changed = true;
                }
            }
            ui.label("Grants are bound to this server address. Load tools again after restarting the app; access tokens stay in memory.");
        }
        kit::section(ui, "Run a tool");
        kit::card(ui, |c| c.block(|ui| crate::mcp_controls::tools_ui(ui, mcp)));
    }
    if changed {
        studio.save();
    }
}

pub(super) fn tasks(ui: &mut egui::Ui, studio: &mut Studio) {
    let proposals = studio.proposals.len();
    kit::section(ui, "Agent tasks");
    kit::card(ui, |c| {
        if let Some(character) = studio.library.selected_character_mut() {
            let name = character.name.clone();
            if c.row(
                "Allow task proposals",
                &format!("{name} can suggest tasks. Review each before an agent runs."),
                |ui| kit::toggle(ui, &mut character.delegation.enabled),
            )
            .changed()
            {
                studio.proposals.clear();
                studio.save();
            }
        }
        c.row(
            "Suggested tasks",
            if proposals == 0 {
                "Nothing waiting."
            } else {
                "Review and run them below."
            },
            |ui| {
                ui.label(proposals.to_string());
            },
        );
    });
}

#[cfg(test)]
mod session_manager_tests {
    use super::*;
    fn frame(
        ctx: &egui::Context,
        studio: &mut Studio,
        listening: &mut ListeningState,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800., 600.),
                )),
                events,
                ..Default::default()
            },
            |ui| session_keys(ui, studio, listening),
        );
        output.textures_delta.clear();
        output
    }
    fn text_rect(shape: &egui::Shape, label: &str) -> Option<egui::Rect> {
        match shape {
            egui::Shape::Text(text) if text.galley.job.text == label => {
                Some(egui::Rect::from_min_size(text.pos, text.galley.size()))
            }
            egui::Shape::Vec(shapes) => shapes.iter().find_map(|shape| text_rect(shape, label)),
            _ => None,
        }
    }
    fn voice_frame(
        ctx: &egui::Context,
        studio: &mut Studio,
        listening: &mut ListeningState,
        size: egui::Vec2,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        let mut speech = ModelCatalog::default();
        let mut transcription = ModelCatalog::default();
        let mut voices = waifu_studio::voice_library::VoiceLibraryState::default();
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                events,
                ..Default::default()
            },
            |ui| {
                voice(
                    ui,
                    studio,
                    &mut speech,
                    &mut transcription,
                    listening,
                    &mut voices,
                    Some(0),
                )
            },
        );
        output.textures_delta.clear();
        output
    }
    fn click_events(pos: egui::Pos2) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ]
    }
    #[test]
    fn voice_output_dropdown_clicks_fit_and_persist_all_modes_per_character() {
        use waifu_studio::voice::VoiceOutput;
        for size in [egui::vec2(1280., 800.), egui::vec2(800., 720.)] {
            let root =
                std::env::temp_dir().join(format!("voice-output-ui-{}", uuid::Uuid::new_v4()));
            let (store, mut library) = waifu_studio::data::Store::open(root.clone()).unwrap();
            let id = uuid::Uuid::new_v4();
            library.characters.push(
                serde_json::from_value(serde_json::json!({
                    "id":id,"name":"Fixture","model":"fixture.vrm","persona":"","format":"vrm"
                }))
                .unwrap(),
            );
            library.selected = Some(id);
            library.voice = VoiceConfig::preset(VoiceAdapter::FishLocal);
            library.voice.endpoint = ":/invalid".into();
            library.voice.api_key_env.clear();
            library.voice.output = VoiceOutput::CallsOnly;
            let mut studio = Studio::new(store, library);
            let mut listening = ListeningState::default();
            listening.config.endpoint = ":/invalid".into();
            listening.config.api_key_env.clear();
            let ctx = egui::Context::default();
            let mut label = "Calls only";
            for (pick, expected) in [
                ("Local only", VoiceOutput::LocalOnly),
                ("Local + calls", VoiceOutput::LocalAndCalls),
                ("Calls only", VoiceOutput::CallsOnly),
            ] {
                let output = voice_frame(&ctx, &mut studio, &mut listening, size, vec![]);
                let rect = output
                    .shapes
                    .iter()
                    .find_map(|shape| text_rect(&shape.shape, label))
                    .unwrap();
                assert!(rect.min.x >= 0. && rect.max.x <= size.x);
                voice_frame(
                    &ctx,
                    &mut studio,
                    &mut listening,
                    size,
                    click_events(rect.center()),
                );
                let output = voice_frame(&ctx, &mut studio, &mut listening, size, vec![]);
                let option = output
                    .shapes
                    .iter()
                    .find_map(|shape| text_rect(&shape.shape, pick))
                    .unwrap();
                assert!(option.min.x >= 0. && option.max.x <= size.x);
                voice_frame(
                    &ctx,
                    &mut studio,
                    &mut listening,
                    size,
                    click_events(option.center()),
                );
                assert_eq!(studio.library.voice.output, expected);
                let (_, saved) = waifu_studio::data::Store::open(root.clone()).unwrap();
                assert_eq!(saved.voice.output, expected);
                assert_eq!(
                    saved
                        .selected_character()
                        .unwrap()
                        .voice
                        .as_ref()
                        .unwrap()
                        .output,
                    expected
                );
                label = pick;
            }
            studio.library.selected = None;
            let output = voice_frame(&ctx, &mut studio, &mut listening, size, vec![]);
            assert!(output.shapes.iter().any(|shape| {
                text_rect(
                    &shape.shape,
                    "Select a character to use call output. Previews stay local.",
                )
                .is_some()
            }));
            drop(studio);
            std::fs::remove_dir_all(root).unwrap();
        }
    }
    fn shows_fixture_secret(shape: &egui::Shape) -> bool {
        match shape {
            egui::Shape::Text(text) => text.galley.job.text.contains("fixture-"),
            egui::Shape::Vec(shapes) => shapes.iter().any(shows_fixture_secret),
            _ => false,
        }
    }
    #[test]
    fn typing_session_keys_routes_to_each_feature_without_painting_or_saving_secrets() {
        let root = std::env::temp_dir().join(format!("session-editor-{}", uuid::Uuid::new_v4()));
        let (store, mut library) = waifu_studio::data::Store::open(root.clone()).unwrap();
        library.provider = 0;
        library.providers[0].session_key.clear();
        library.decision.session_key.clear();
        let mut studio = Studio::new(store, library);
        studio.voice_key.clear();
        let mut listening = ListeningState::default();
        let ctx = egui::Context::default();
        frame(&ctx, &mut studio, &mut listening, vec![]);
        for (label, key) in [
            ("Chat", "fixture-chat-key"),
            ("Decision model", "fixture-decision-key"),
            ("Voice", "fixture-voice-key"),
            ("Microphone", "fixture-microphone-key"),
        ] {
            let output = frame(&ctx, &mut studio, &mut listening, vec![]);
            let rect = output
                .shapes
                .iter()
                .find_map(|shape| text_rect(&shape.shape, label))
                .unwrap();
            let pos = egui::pos2(700., rect.center().y);
            frame(
                &ctx,
                &mut studio,
                &mut listening,
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: egui::Modifiers::NONE,
                    },
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
            let output = frame(
                &ctx,
                &mut studio,
                &mut listening,
                vec![egui::Event::Text(key.into())],
            );
            assert!(
                !output
                    .shapes
                    .iter()
                    .any(|shape| shows_fixture_secret(&shape.shape)),
                "secret text must stay masked"
            );
        }
        assert_eq!(studio.library.providers[0].session_key, "fixture-chat-key");
        assert_eq!(studio.library.decision.session_key, "fixture-decision-key");
        assert_eq!(studio.voice_key, "fixture-voice-key");
        assert_eq!(listening.session_key, "fixture-microphone-key");
        studio.store.save(&studio.library).unwrap();
        let saved = std::fs::read_to_string(root.join("library.json")).unwrap();
        assert!(!saved.contains("fixture-"));
        assert!(
            !serde_json::to_string(&listening.config)
                .unwrap()
                .contains("fixture-")
        );
        drop(studio);
        std::fs::remove_dir_all(root).unwrap();
    }
}
