//! Simple first screens for Voice & mic, Scene & avatar and Connections. Each keeps the
//! full original controls under an Advanced fold drawn by the caller.
use super::{
    AvatarUi, Studio, ViewMode, decision_model_compact, model_dropdown, settings_kit as kit,
};
use bevy_egui::egui::{self, Color32, RichText};
use waifu_studio::{
    listen::{ListeningState, SttBackend},
    model_catalog::{CatalogKind, ModelCatalog},
    voice::{VoiceAdapter, VoiceConfig},
};

/// Closed fold holding a page's complete original controls.
pub(super) fn advanced(ui: &mut egui::Ui, id: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.add_space(18.0);
    egui::CollapsingHeader::new(RichText::new("Advanced").size(14.0))
        .id_salt((id, "advanced"))
        .default_open(false)
        .show(ui, add);
}

fn dropdown(ui: &mut egui::Ui, id: &str, current: usize, labels: &[&str]) -> usize {
    let mut selected = current;
    egui::ComboBox::from_id_salt(id)
        .selected_text(labels.get(current).copied().unwrap_or("—"))
        .width(200.0)
        .show_ui(ui, |ui| {
            for (index, label) in labels.iter().enumerate() {
                ui.selectable_value(&mut selected, index, *label);
            }
        });
    selected
}

fn slider(ui: &mut egui::Ui, value: &mut f32, range: std::ops::RangeInclusive<f32>) -> bool {
    ui.spacing_mut().slider_width = 160.0;
    ui.add(egui::Slider::new(value, range).max_decimals(2))
        .changed()
}

fn compact_models(
    ui: &mut egui::Ui,
    salt: &str,
    catalog: &ModelCatalog,
    model: &mut String,
) -> bool {
    ui.set_max_width(260.0);
    let picked = model_dropdown(ui, salt, &catalog.models, model);
    if catalog.loading {
        ui.spinner();
    } else if catalog.models.is_empty() {
        ui.label(RichText::new("⚠").color(super::ACCENT))
            .on_hover_text(&catalog.status);
    }
    match picked {
        Some(picked) if picked != *model => {
            *model = picked;
            true
        }
        _ => false,
    }
}

pub(super) fn voice(
    ui: &mut egui::Ui,
    studio: &mut Studio,
    speech: &mut ModelCatalog,
    transcription: &mut ModelCatalog,
    listening: &mut ListeningState,
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
        let picked = c.row("Voice service", "", |ui| {
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
            studio.library.voice = VoiceConfig::preset(adapters[picked]);
            studio.library.voice.enabled = enabled;
            studio.voice_key.clear();
            changed = true;
        }
        let voice = &mut studio.library.voice;
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
        changed |= c.row("Speed", "", |ui| slider(ui, &mut voice.speed, 0.5..=2.0));
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

    kit::section(ui, "Microphone");
    kit::card(ui, |c| {
        let mut changed = false;
        let mut enabled = listening.config.enabled;
        if c.row(
            "Talk with your microphone",
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
                "Send when you stop talking",
                "Otherwise the transcript waits in the message box for you.",
                |ui| kit::toggle(ui, &mut listening.config.auto_send),
            )
            .changed();
        let openrouter = listening.config.backend == SttBackend::OpenAiCompatible
            && listening.config.endpoint.contains("openrouter.ai/");
        let current = match listening.config.backend {
            SttBackend::VercelGateway => 0,
            SttBackend::OpenAiCompatible if openrouter => 1,
            SttBackend::FishAsr => 2,
            SttBackend::OpenAiCompatible => 3,
        };
        let picked = c.row("Transcription service", "", |ui| {
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
        });
        if picked != current {
            listening.cancel();
            match picked {
                0 => listening.config.preset(SttBackend::VercelGateway),
                1 => listening.config.openrouter_preset(),
                2 => listening.config.preset(SttBackend::FishAsr),
                _ => listening.config.preset(SttBackend::OpenAiCompatible),
            }
            changed = true;
        }
        let config = &mut listening.config;
        transcription.sync(
            &config.endpoint,
            &config.api_key_env,
            "",
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

pub(super) fn scene(ui: &mut egui::Ui, studio: &mut Studio, avatar: &mut AvatarUi) {
    kit::section(ui, "Look");
    kit::card(ui, |c| {
        let scene = &mut studio.library.scene;
        let mut changed = false;
        let shader = c.row("Shading", "", |ui| {
            dropdown(
                ui,
                "simple_shader",
                scene.shader.min(2) as usize,
                &["Anime (MToon)", "Realistic (PBR)", "Flat (unlit)"],
            )
        });
        if shader != scene.shader as usize {
            scene.shader = shader as u8;
            changed = true;
        }
        let framing = c.row("Camera", "", |ui| {
            dropdown(
                ui,
                "simple_framing",
                usize::from(!scene.portrait),
                &["Portrait", "Full body"],
            )
        });
        if framing != usize::from(!scene.portrait) {
            scene.portrait = framing == 0;
            studio.frame_requested = true;
            changed = true;
        }
        changed |= c
            .row(
                "Outlines",
                "Anime-style ink lines around the character.",
                |ui| kit::toggle(ui, &mut scene.outlines),
            )
            .changed();
        changed |= c
            .row("Idle movement", "Gentle breathing and sway.", |ui| {
                kit::toggle(ui, &mut scene.idle_motion)
            })
            .changed();
        if changed {
            studio.save();
        }
    });

    kit::section(ui, "Behavior");
    kit::card(ui, |c| {
        let mut changed = false;
        let scene = &mut studio.library.scene;
        changed |= c
            .row(
                "Expressions",
                "Looks thoughtful while thinking and animated while speaking.",
                |ui| kit::toggle(ui, &mut scene.behavior.enabled),
            )
            .changed();
        let mut emotions = scene.affect.enabled;
        if c.row(
            "Emotions from chat",
            "Reacts to the conversation with matching faces and moods.",
            |ui| kit::toggle(ui, &mut emotions),
        )
        .changed()
        {
            scene.affect.enabled = emotions;
            studio.cancel_appraisal();
            changed = true;
        }
        let scene = &mut studio.library.scene;
        changed |= c
            .row(
                "Head and eye movement",
                "Looks around naturally, or follows your pointer.",
                |ui| kit::toggle(ui, &mut avatar.gaze.config.enabled),
            )
            .changed();
        changed |= c
            .row("Blinking", "", |ui| {
                kit::toggle(ui, &mut avatar.facial.config.auto_blink)
            })
            .changed();
        changed |= c
            .row("Lip sync", "Moves the mouth with speech.", |ui| {
                kit::toggle(ui, &mut avatar.facial.config.lip_sync)
            })
            .changed();
        changed |= c
            .row(
                "Daily routine",
                "Imagined activities through the week that can come up in chat.",
                |ui| kit::toggle(ui, &mut scene.routine.enabled),
            )
            .changed();
        use waifu_studio::data::ReplyMotionMode;
        let modes = [
            ReplyMotionMode::Off,
            ReplyMotionMode::Suggest,
            ReplyMotionMode::AutoPlay,
            ReplyMotionMode::EmotionMapped,
        ];
        let current = modes
            .iter()
            .position(|m| *m == scene.reply_motion.mode)
            .unwrap_or(0);
        let picked = c.row(
            "Body motion after replies",
            "Uses animations from your motion library.",
            |ui| {
                dropdown(
                    ui,
                    "simple_reply_motion",
                    current,
                    &["Off", "Suggest", "Play automatically", "Match emotion"],
                )
            },
        );
        if picked != current {
            scene.reply_motion.mode = modes[picked];
            changed = true;
        }
        if changed {
            studio.library.scene.gaze = avatar.gaze.config.clone();
            studio.library.scene.facial = avatar.facial.config.clone();
            studio.save();
        }
        let connection = studio.library.decision.clone();
        let mut model = connection.model.clone();
        c.row(
            "Decision model",
            "Picks emotions and motions when those need a choice.",
            |ui| {
                decision_model_compact(
                    ui,
                    "scene_decision_model",
                    &mut studio.decision_models,
                    &connection,
                    &mut model,
                )
            },
        );
        if model != studio.library.decision.model {
            studio.library.decision.model = model;
            studio.cancel_appraisal();
            studio.save();
        }
    });

    kit::section(ui, "Desktop");
    kit::card(ui, |c| {
        if c.row(
            "Desktop companion",
            "Puts the character on your desktop in a transparent window.",
            |ui| ui.button("Open").clicked(),
        ) {
            studio.mode = ViewMode::Desktop;
        }
    });
}

pub(super) fn connections(ui: &mut egui::Ui, studio: &mut Studio) {
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
            ui.set_max_width(260.0);
            let picked = model_dropdown(ui, "simple_connection_models", &models, &current);
            if loading {
                ui.spinner();
            }
            picked
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
            "Small, fast choices: memories, emotions, motions, who replies.",
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
    let mut keys: Vec<(&str, String)> = Vec::new();
    let mut add = |label: &'static str, env: &str| {
        let env = env.trim();
        if !env.is_empty() && !keys.iter().any(|(_, e)| e == env) {
            keys.push((label, env.to_owned()));
        }
    };
    if let Some(provider) = studio.library.providers.get(studio.library.provider) {
        add("Chat", &provider.key_env);
    }
    add("Decision model", &studio.library.decision.key_env);
    add("Voice", &studio.library.voice.api_key_env);
    add("Microphone", &studio.library.listen.api_key_env);
    kit::card(ui, |c| {
        for (label, env) in &keys {
            let found = std::env::var_os(env).is_some_and(|v| !v.is_empty());
            c.row(label, env, |ui| {
                if found {
                    ui.label(RichText::new("Ready").color(Color32::from_rgb(120, 190, 140)));
                } else {
                    ui.label(RichText::new("Missing").color(super::ACCENT))
                        .on_hover_text("Add it to credentials.env in the app's data folder, or paste a session key under Advanced.");
                }
            });
        }
    });
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
                            .width(200.0)
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
            ui.horizontal(|ui| {
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

pub(super) fn tools(ui: &mut egui::Ui, mcp: &mut waifu_studio::mcp::McpState) {
    let busy = mcp.is_busy();
    kit::section(ui, "Tool server");
    kit::card(ui, |c| {
        c.row(
            "Server address",
            "An MCP server reachable over HTTP.",
            |ui| {
                ui.add_enabled_ui(!busy, |ui| {
                    if ui
                        .add(egui::TextEdit::singleline(&mut mcp.endpoint).desired_width(240.0))
                        .changed()
                    {
                        mcp.disarm();
                    }
                });
            },
        );
        c.row(
            "Access token",
            "Optional. Kept for this session only.",
            |ui| {
                ui.add_enabled(
                    !busy,
                    egui::TextEdit::singleline(&mut mcp.session_token)
                        .password(true)
                        .desired_width(240.0),
                );
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
    });
    if !mcp.tools.is_empty() {
        kit::section(ui, "Run a tool");
        kit::card(ui, |c| c.block(|ui| crate::mcp_controls::tools_ui(ui, mcp)));
    }
}

pub(super) fn tasks(ui: &mut egui::Ui, studio: &mut Studio) {
    let proposals = studio.proposals.len();
    kit::section(ui, "Agent tasks");
    kit::card(ui, |c| {
        if let Some(character) = studio.library.selected_character_mut() {
            let name = character.name.clone();
            if c.row(
                &format!("Let {name} suggest coding tasks"),
                "Suggestions wait for your review before any agent runs.",
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
                "Review them under Advanced."
            },
            |ui| {
                ui.label(proposals.to_string());
            },
        );
    });
}
