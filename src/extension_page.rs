//! Compact content and local integration management; installation runs in the worker.
use super::Studio;
#[cfg(test)]
use super::{companion_shared, lock, motion_library};
use bevy_egui::egui;
use std::collections::BTreeMap;
#[cfg(test)]
use std::{path::PathBuf, sync::Arc};
use waifu_studio::{
    animation::AnimationState,
    behavior::reserved_expression,
    extension_manifest::{BackgroundSource, ContentKind, ResolvedPack, RoomId},
    extension_protocol::{Capability, MAX_ID_BYTES, MAX_REGISTERED_ID_BYTES},
    extension_runtime::ExtensionRuntime,
    extension_settings::{SettingDefinition, SettingKind, SettingValue, Settings, validate_values},
    extensions::{ExtensionState, PackView},
    settings_kit as kit,
};

fn bundled_example_button(ui: &mut egui::Ui, label: &str, directory: &std::path::Path) -> bool {
    ui.add_enabled(
        directory.join("extension.json").is_file(),
        egui::Button::new(label),
    )
    .on_disabled_hover_text("Not included in this installation.")
    .clicked()
}

#[cfg(test)]
fn motions(ctx: &egui::Context) -> Arc<Vec<motion_library::MotionEntry>> {
    super::motion_service_handles(ctx).extension_entries()
}
#[cfg(test)]
fn poll(ctx: &egui::Context, extensions: &mut ExtensionState, animation: &mut AnimationState) {
    extensions.poll();
    super::motion_service_handles(ctx).sync_extensions(extensions, animation);
}

/// Runtime-only entries have no installed record to persist their enabled state.
/// Installed integrations are controlled exclusively by their installed cards.
pub fn runtime_ui(ui: &mut egui::Ui, runtime: &ExtensionRuntime, extensions: &ExtensionState) {
    let entries: Vec<_> = runtime
        .views()
        .into_iter()
        .filter(|entry| {
            entry.test_parent.is_none()
                && !extensions
                    .packs
                    .iter()
                    .any(|pack| pack.installed.installation_id == entry.id)
        })
        .collect();
    if entries.is_empty() {
        return;
    }
    kit::section(ui, "Other local integrations");
    for entry in entries {
        ui.push_id(("runtime-only", entry.id), |ui| {
            kit::card(ui, |card| {
                card.row(&entry.name, &entry.status, |ui| {
                    if entry.enabled {
                        if ui.small_button("Stop").clicked() {
                            runtime.stop(entry.id);
                        }
                    } else if ui.small_button("Start").clicked() {
                        runtime.restart(entry.id);
                    }
                    if ui
                        .add_enabled(entry.enabled, egui::Button::new("Restart").small())
                        .clicked()
                    {
                        runtime.restart(entry.id);
                    }
                });
            });
        });
    }
}

fn call_output_guidance(
    enabled: bool,
    allowed: bool,
    active: bool,
    ready: Option<bool>,
    has_character: bool,
) -> &'static str {
    if ready.is_none() {
        "Call audio is unavailable in this session."
    } else if !allowed {
        "Allow speech playback in Permissions."
    } else if !enabled {
        "Enable this integration to connect."
    } else if !active {
        "Complete setup, then restart this integration."
    } else if !has_character {
        "Select a character to use call output."
    } else if ready == Some(true) {
        "Ready for this character's new speech."
    } else {
        "Waiting for the call connection to become ready."
    }
}

pub fn ui(
    ui: &mut egui::Ui,
    studio: &mut Studio,
    extensions: &mut ExtensionState,
    animation: &mut AnimationState,
    runtime: Option<&ExtensionRuntime>,
    available_expressions: &[String],
    ready_calls: Option<&[uuid::Uuid]>,
) {
    kit::section(ui, "Extensions");
    kit::card(ui, |card| {
        card.row(
            "Add extensions",
            "Install content or connect an integration.",
            |ui| {
                ui.add_enabled_ui(!extensions.busy, |ui| {
                    ui.menu_button("Add…", |ui| {
                        super::popup_layout(ui, 280.0);
                        if ui.button("Content pack from folder…").clicked() {
                            extensions.pick_install();
                            ui.close();
                        }
                        if ui
                            .button("Local integration from folder…")
                            .on_hover_text("Runs local code. Review permissions before enabling.")
                            .clicked()
                        {
                            extensions.pick_install_integration();
                            ui.close();
                        }
                        let base = waifu_studio::bundled::extension_examples_path();
                        if bundled_example_button(
                            ui,
                            "Companion starter motions",
                            &base.join("companion-starter"),
                        ) {
                            extensions.install(base.join("companion-starter"));
                            ui.close();
                        }
                        if base.join("twitch-live/extension.json").is_file()
                            && ui.button("Twitch").clicked()
                        {
                            extensions.install_integration(base.join("twitch-live"));
                            ui.close();
                        }
                        ui.menu_button("Examples", |ui| {
                            super::popup_layout(ui, 280.0);
                            for (label, folder) in [
                                ("Lifecycle observer", "lifecycle-observer"),
                                ("JavaScript observer", "javascript-observer"),
                                ("Reply gesture", "reply-gesture"),
                                ("Reply gesture hook", "hook-gesture"),
                                ("Configurable reaction hook", "reaction-hook"),
                                ("Emotion hook", "emotion-hook"),
                                ("Character selection observer", "selection-observer"),
                                ("Startup observer", "startup-observer"),
                                ("Motion end observer", "motion-end-observer"),
                                ("Motion activation observer", "motion-start-observer"),
                                ("Integration exit observer", "integration-stop-observer"),
                                ("Integration startup observer", "integration-start-observer"),
                                ("Discord replay (offline)", "discord-replay"),
                                ("Twitch replay (offline)", "twitch-replay"),
                            ] {
                                if bundled_example_button(ui, label, &base.join(folder)) {
                                    extensions.install_integration(base.join(folder));
                                    ui.close();
                                }
                            }
                        });
                    });
                });
            },
        );
    });
    ui.horizontal_wrapped(|ui| {
        if extensions.busy {
            ui.spinner();
        }
        kit::note(ui, &extensions.status);
        if ui
            .add_enabled(!extensions.busy, egui::Button::new("Refresh"))
            .clicked()
        {
            extensions.refresh();
        }
    });
    if let Some(error) = &extensions.error {
        ui.colored_label(super::ACCENT, error);
    }
    if extensions.packs.is_empty() && !extensions.busy {
        kit::note(
            ui,
            "Installed packs and integrations appear here. Review integration permissions before enabling local code.",
        );
    }
    kit::section(ui, "Incoming messages");
    let reply_policy_changed = kit::card(ui, |card| {
        let mut changed = card
            .row(
                "Reply to incoming messages",
                "Let the character respond using its chat and voice settings.",
                |ui| {
                    let selected = studio.library.selected_character().is_some();
                    ui.add_enabled_ui(selected, |ui| {
                        kit::toggle(ui, &mut studio.library.scene.reply_to_external_messages)
                    })
                    .inner
                },
            )
            .changed();
        if studio.library.scene.reply_to_external_messages {
            changed |= card
                .row(
                    "Pick chat highlights",
                    "Choose up to two messages per chunk using the decision model.",
                    |ui| kit::toggle(ui, &mut studio.library.scene.select_incoming_messages),
                )
                .changed();
            if studio.library.scene.select_incoming_messages {
                let connection = studio.library.decision.clone();
                let mut model = connection.model.clone();
                card.row(
                    "Decision model",
                    "Uses your existing decision connection. Direct chat bypasses selection.",
                    |ui| {
                        super::decision_model_compact(
                            ui,
                            "incoming_decision_model",
                            &mut studio.decision_models,
                            &connection,
                            &mut model,
                        );
                    },
                );
                if model != studio.library.decision.model {
                    studio.library.decision.model = model;
                    changed = true;
                }
            }
        }
        changed
    });
    if reply_policy_changed {
        studio.save();
    }
    let runtime_entries = runtime.map(ExtensionRuntime::views).unwrap_or_default();
    let motion_choices = registered_motion_choices(extensions.active());
    let expression_choices = available_expression_choices(available_expressions);
    let character_choices = available_character_choices(&studio.library.characters);
    // Work against a snapshot; commands return an updated catalog asynchronously.
    for pack in extensions.packs.clone() {
        let id = pack.installed.installation_id;
        let executable = pack.installed.executable;
        let hooks = pack
            .content
            .as_ref()
            .is_some_and(|content| !content.manifest.hooks.is_empty());
        let entry = runtime_entries.iter().find(|entry| entry.id == id);
        ui.push_id(id, |ui| {
            kit::section(ui, &pack.installed.name);
            kit::card(ui, |card| {
                let state = if pack.installed.enabled { "Enabled" } else { "Disabled" };
                let status = if pack.installed.enabled {
                    entry.map(|entry| entry.status.as_str()).unwrap_or(state)
                } else { state };
                card.row("Enabled", status, |ui| {
                    let mut enabled = pack.installed.enabled;
                    let can_change = !extensions.busy && (enabled ||
                        (pack.error.is_none() && (!executable || runtime.is_some())));
                    if ui.add_enabled_ui(can_change, |ui| kit::toggle(ui, &mut enabled))
                        .inner.changed() {
                        extensions.set_enabled(id, enabled);
                    }
                });
                if pack.content.as_ref().is_some_and(|content|
                    content.manifest.capabilities.iter().any(|cap| cap == "audio.playback"))
                {
                    let allowed = pack.installed.grants.contains(&Capability::AudioPlayback);
                    let ready = ready_calls.map(|ids| ids.contains(&id));
                    let active = entry.is_some_and(|entry| entry.active);
                    let guidance = call_output_guidance(pack.installed.enabled, allowed, active, ready, studio.library.selected.is_some());
                    let can_play = pack.installed.enabled && allowed && active && studio.library.selected.is_some() && ready == Some(true);
                    card.row("Call output", guidance, |ui| {
                        ui.weak(if can_play { "Ready" } else { "Not ready" });
                    });
                }
                if let Some(error) = &pack.error {
                    card.block(|ui| { ui.colored_label(super::ACCENT, error); });
                }
                if let Some(content) = &pack.content {
                    card.block(|ui| {
                        if !content.manifest.description.is_empty() {
                            kit::note(ui, &content.manifest.description);
                        }
                        let options: Vec<_> = content.content.iter()
                            .filter(|c| c.kind != ContentKind::Expression).collect();
                        if !options.is_empty() {
                            let selection_id = ui.id().with("content-selection");
                            let mut selected = ui.ctx().data(|d| d.get_temp::<String>(selection_id))
                                .filter(|id| options.iter().any(|c| c.id == *id))
                                .or_else(|| options.first().map(|c| c.id.clone())).unwrap_or_default();
                            ui.add_enabled_ui(pack.installed.enabled && !extensions.busy && pack.error.is_none(), |ui| {
                                kit::row(ui, "Use content", "Choose an item from this pack.", |ui| {
                                    ui.horizontal_wrapped(|ui| {
                                    if ui.add_enabled(!selected.is_empty(), egui::Button::new("Use").small()).clicked()
                                        && let Err(error) = apply(ui.ctx(), studio, animation, content, &selected) {
                                            studio.error = Some(error);
                                        }
                                    egui::ComboBox::from_id_salt("content").width(ui.available_width().min(180.0))
                                        .selected_text(content_label(content, &selected)).truncate().show_ui(ui, |ui| {
                                            super::popup_layout(ui, 340.0);
                                            for option in &options {
                                                ui.selectable_value(&mut selected, option.id.clone(), content_label(content, &option.id));
                                            }
                                        });
                                    });
                                });
                            });
                            ui.ctx().data_mut(|d| d.insert_temp(selection_id, selected));
                        }
                        if !content.manifest.content.expressions.is_empty() {
                            kit::note(ui, "Expression mappings follow enabled reply emotion and available avatar expressions.");
                        }
                    });
                }
                if pack.content.as_ref().is_some_and(|content| !content.manifest.settings.is_empty()) {
                    card.block(|ui| settings_ui(ui, extensions, &pack, &motion_choices, &expression_choices, &character_choices));
                }
                if pack.content.as_ref().is_some_and(|content| !content.manifest.credentials.is_empty()) {
                    card.block(|ui| credentials_ui(ui, extensions, &pack));
                }
                if executable {
                    card.block(|ui| permissions(ui, extensions, &pack));
                }
                card.block(|ui| {
                    egui::CollapsingHeader::new("Advanced").default_open(false).show_unindented(ui, |ui| {
                if executable {
                    let fallback_status = if !pack.installed.enabled {
                        if hooks { "Disabled · enable to listen for events" }
                        else { "Disabled · enable to run local code" }
                    } else if runtime.is_none() {
                        "Integration runtime unavailable"
                    } else {
                        "Waiting for runtime"
                    };
                    let status = entry.map(|entry| entry.status.as_str()).unwrap_or(fallback_status);
                    let control_label = if hooks { "Reset" } else { "Restart" };
                    let control_hint = if hooks {
                        "Discards queued invocations, stops any running hook, and waits for new events."
                    } else {
                        "Stops and restarts the integration process."
                    };
                    kit::row(ui, if hooks { "Hooks" } else { "Process" }, status, |ui| {
                        if ui.add_enabled(!extensions.busy && pack.installed.enabled
                            && pack.error.is_none() && entry.is_some(),
                            egui::Button::new(control_label).small()).on_hover_text(control_hint).clicked()
                            && let Some(runtime) = runtime {
                                runtime.restart(id);
                            }
                    });
                    if pack.content.as_ref().is_some_and(|content| content.manifest.test_process.is_some()) {
                        let test_active = runtime.is_some_and(|runtime| runtime.test_active(id));
                        let test_status = runtime_entries.iter()
                            .find(|entry| entry.test_parent == Some(id) && entry.active)
                            .map(|entry| entry.status.clone())
                            .or_else(|| runtime.and_then(|runtime| runtime.test_status(id)));
                        kit::row(ui, "Saved reaction", "Uses saved settings and the loaded character; unsaved drafts are not used.", |ui| {
                            let can_test = !extensions.busy && pack.installed.enabled
                                && pack.error.is_none() && runtime.is_some() && entry.is_some()
                                && !test_active;
                            if ui.add_enabled(can_test, egui::Button::new("Test reaction").small())
                                .on_hover_text(if test_active { "A test reaction is already running." }
                                    else { "Runs this integration's declared test process with its saved settings and current permissions." })
                                .clicked() && let Some(runtime) = runtime
                                && let Err(error) = runtime.start_test(id) {
                                    studio.error = Some(error.to_string());
                                }
                        });
                        if let Some(test_status) = test_status {
                            kit::row(ui, "Test result", &test_status, |_| {});
                        }
                    }
                }
                        if let Some(content) = &pack.content {
                            kit::note(ui, format!("{} · {}", content.manifest.provenance.author, content.manifest.provenance.license));
                            if hooks {
                                let triggers = content.manifest.hooks.iter()
                                    .map(|hook| hook_event_label(&hook.event)).collect::<Vec<_>>().join(", ");
                                kit::note(ui, format!("Triggers: {triggers}"));
                            }
                        }
                        kit::note(ui, format!("Version {} · {}", pack.installed.version, pack.installed.package_id));
                        if let Some(entry) = entry.filter(|entry| !entry.logs.is_empty()) {
                            ui.collapsing("Diagnostics", |ui| {
                                egui::ScrollArea::vertical().id_salt("extension-diagnostics").max_height(200.0).show(ui, |ui| {
                                // The runtime supplies redacted strings. Render only
                                // bounded plain text, never Debug or rich markup.
                                for line in entry.logs.iter().skip(entry.logs.len().saturating_sub(8)) {
                                    let text: String = line.chars().take(1024).collect();
                                    ui.add(egui::Label::new(text).wrap());
                                }
                                });
                            });
                        }
                        if executable {
                            kit::note(ui, "Runs local code without a sandbox. Disabling stops the integration. Removal unregisters it; installed files and integration data are retained.");
                        } else {
                            kit::note(ui, "Removal unregisters this pack. Files are retained for characters and settings you already imported.");
                        }
                        remove_ui(ui, extensions, id);
                    });
                });
            });
        });
    }
    if let Some(runtime) = runtime {
        runtime_ui(ui, runtime, extensions);
    }
    if let Some(error) = &studio.error {
        ui.colored_label(super::ACCENT, error);
    }
    if let Some(notice) = &studio.notice {
        kit::note(ui, notice);
    }
}

fn hook_event_label(event: &str) -> &str {
    match event {
        "extension.stopping" => "Integration stopping",
        "extension.stopped" => "Integration stopped",
        "motion.playback.started" => "Motion activated",
        "motion.playback.ended" => "Motion ended",
        "extension.started" => "Integration started",
        "app.ready" => "App ready",
        "character.selected" => "Character selected",
        "emotion.changed" => "Emotion changed",
        "reply.finished" => "Reply finished",
        "speech.started" => "Speech started",
        "speech.stopped" => "Speech stopped",
        _ => event,
    }
}

// Protocol names stay out of ordinary labels; descriptions state the host API
// access being granted, without implying restrictions on unsandboxed local code.
const PERMISSIONS: [(Capability, &str, &str, &str); 9] = [
    (
        Capability::AudioPlayback,
        "Receive speech audio",
        "Receive synthesized character speech for external playback. Revoking stops this integration; restart it explicitly to reconnect.",
        "audio.playback",
    ),
    (
        Capability::AudioTranscribe,
        "Transcribe participant audio",
        "Submit participant audio to the configured speech recognition service. Also requires Deliver messages; voice intake is experimental.",
        "audio.transcribe",
    ),
    (
        Capability::MotionPlay,
        "Play motions",
        "Play registered motions once.",
        "motion.play",
    ),
    (
        Capability::ExpressionApply,
        "Apply expressions",
        "Apply an available avatar expression for a bounded duration.",
        "expression.apply",
    ),
    (
        Capability::MessageDeliver,
        "Deliver messages",
        "Add attributed external context. Replies require the separate incoming-message opt-in.",
        "message.deliver",
    ),
    (
        Capability::EventEmit,
        "Emit integration events",
        "Emit events in this integration's own namespace.",
        "event.emit",
    ),
    (
        Capability::CatalogGet,
        "Read available content",
        "Read the host catalog of registered motions and available expressions.",
        "catalog.get",
    ),
    (
        Capability::LifecycleObserve,
        "Observe reply and speech activity",
        "Receive source identities and completion/playback states, without reply text.",
        "lifecycle.observe",
    ),
    (
        Capability::ReplyRead,
        "Read reply text",
        "Receive completed reply text. Reply completion and speech activity are controlled separately.",
        "reply.read",
    ),
];

fn permissions(ui: &mut egui::Ui, extensions: &mut ExtensionState, pack: &PackView) {
    kit::row(
        ui,
        "Permissions",
        "App actions and events; local code is not sandboxed.",
        |ui| {
            let mut grants = pack.installed.grants.clone();
            let mut changed = false;
            ui.add_enabled_ui(!extensions.busy && pack.error.is_none(), |ui| {
                egui::ComboBox::from_id_salt("permissions")
                    .selected_text(format!("{} allowed", grants.len()))
                    .width(ui.available_width().min(180.0))
                    .truncate()
                    .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                    .show_ui(ui, |ui| {
                        super::popup_layout(ui, 340.0);
                        for (capability, label, description, name) in PERMISSIONS {
                            let mut granted = grants.contains(&capability);
                            let requested = pack.content.as_ref().is_some_and(|content| {
                                content.manifest.capabilities.iter().any(|c| c == name)
                            });
                            let text = if requested || granted {
                                label.to_owned()
                            } else {
                                format!("{label} · not requested")
                            };
                            if ui
                                .add_enabled_ui(requested || granted, |ui| {
                                    ui.checkbox(&mut granted, text).on_hover_text(description)
                                })
                                .inner
                                .changed()
                            {
                                if granted {
                                    grants.push(capability);
                                } else {
                                    grants.retain(|grant| *grant != capability);
                                }
                                changed = true;
                            }
                        }
                    });
            });
            if changed {
                extensions.set_grants(pack.installed.installation_id, grants);
            }
        },
    );
}

// Transient UI state only: deliberately no Debug or serialization. Never seed
// entries from saved credentials; the worker exposes configured IDs only.
#[derive(Clone, Default)]
struct CredentialsDraft {
    install_revision: Option<uuid::Uuid>,
    credential_revision: Option<uuid::Uuid>,
    values: BTreeMap<String, String>,
}

impl CredentialsDraft {
    fn take_updates(&mut self) -> BTreeMap<String, Option<String>> {
        std::mem::take(&mut self.values)
            .into_iter()
            .filter(|(_, value)| !value.is_empty())
            .map(|(id, value)| (id, Some(value)))
            .collect()
    }
}

fn valid_twitch_client_id(value: &str) -> bool {
    waifu_studio::twitch_auth::valid_client_id(value)
}

fn twitch_auth_can_connect(client_id: &str, stored: bool) -> bool {
    valid_twitch_client_id(client_id) || (client_id.is_empty() && stored)
}

fn twitch_connect_control(
    ui: &mut egui::Ui,
    client_id: &str,
    stored: bool,
    available: bool,
) -> egui::Response {
    kit::row(
        ui,
        "Twitch sign-in",
        if client_id.is_empty() && stored {
            "Uses your saved application ID. Connect to authorize this Twitch account."
        } else {
            "Enter your Twitch application ID below, then Connect. No client secret is needed."
        },
        |ui| {
            ui.add_enabled(
                available && twitch_auth_can_connect(client_id, stored),
                egui::Button::new("Connect with Twitch").small(),
            )
        },
    )
}

fn twitch_sign_in_ui(
    ui: &mut egui::Ui,
    extensions: &mut ExtensionState,
    pack: &PackView,
    draft: &mut CredentialsDraft,
) {
    let id = pack.installed.installation_id;
    // Status contains fixed messages and a transient consent challenge only;
    // never cache it in egui storage or format the view with Debug.
    let auth = extensions.twitch_auth_status(id);
    let active = auth.as_ref().is_some_and(|view| view.active);
    let client_id = draft.values.get("client_id").map_or("", String::as_str);
    let valid_client_id = valid_twitch_client_id(client_id);
    let stored = pack.credentials.configured.contains("client_id");
    let connect = twitch_connect_control(
        ui,
        client_id,
        stored,
        !extensions.busy && pack.error.is_none() && !active,
    )
    .clicked();
    if !client_id.is_empty() && !valid_client_id {
        kit::note(
            ui,
            "The application ID must contain 1–128 ASCII letters or digits.",
        );
    }
    if connect {
        // An empty ID asks the store worker to reuse this installation's saved
        // value. Manual token drafts never enter authorization; clear on submit.
        let client_id = draft.values.remove("client_id").unwrap_or_default();
        draft.values.clear();
        if let Err(error) = extensions.start_twitch_auth(id, client_id) {
            extensions.error = Some(error);
        }
    }
    if let Some(auth) = auth {
        ui.horizontal_wrapped(|ui| {
            ui.label(&auth.status);
            if auth.active && ui.small_button("Cancel").clicked() {
                extensions.cancel_twitch_auth(auth.attempt_id);
            }
        });
        if auth.active
            && let Some(challenge) = &auth.challenge
        {
            if challenge.expires_at > std::time::Instant::now() {
                ui.horizontal_wrapped(|ui| {
                    ui.label("Authorization code:");
                    ui.monospace(&challenge.user_code);
                    if ui.small_button("Open browser").clicked() {
                        ui.ctx()
                            .open_url(egui::OpenUrl::new_tab(challenge.verification_uri.clone()));
                    }
                });
            } else {
                kit::note(
                    ui,
                    "Authorization code expired. Cancel this attempt to connect again.",
                );
            }
        }
    }
    kit::note(
        ui,
        "Sign-in saves account credentials. Check the integration's Process status separately.",
    );
}

fn credentials_ui(ui: &mut egui::Ui, extensions: &mut ExtensionState, pack: &PackView) {
    let Some(content) = &pack.content else {
        return;
    };
    let definitions = &content.manifest.credentials;
    let draft_id = ui.id().with("credentials-draft");
    // Move instead of cloning credential strings on every frame. insert_temp
    // is not persisted to egui's serialized settings.
    let mut draft = ui
        .ctx()
        .data_mut(|data| data.remove_temp::<CredentialsDraft>(draft_id))
        .filter(|draft| {
            draft.install_revision == Some(pack.installed.revision)
                && draft.credential_revision == pack.credentials.revision
        })
        .unwrap_or_else(|| CredentialsDraft {
            install_revision: Some(pack.installed.revision),
            credential_revision: pack.credentials.revision,
            values: BTreeMap::new(),
        });
    draft
        .values
        .retain(|id, _| definitions.iter().any(|definition| definition.id == *id));
    let capture = ui
        .ctx()
        .data(|data| data.get_temp::<bool>(egui::Id::new("capture-advanced")))
        .unwrap_or(false);
    let mut clear = None;
    let mut save = false;
    let twitch =
        pack.installed.executable && pack.installed.package_id == "studio.integration.twitch";
    let section = egui::CollapsingHeader::new(if twitch {
        "Twitch account"
    } else {
        "Credentials"
    })
    .default_open(capture)
    .show_unindented(ui, |ui| {
        kit::note(
            ui,
            "Saved privately on this device. Empty fields keep existing values.",
        );
        if twitch {
            twitch_sign_in_ui(ui, extensions, pack, &mut draft);
        }
        ui.add_enabled_ui(!extensions.busy && pack.error.is_none(), |ui| {
            let mut fields = |ui: &mut egui::Ui, manual: bool| {
                for definition in definitions {
                    let token = twitch
                        && matches!(definition.id.as_str(), "access_token" | "refresh_token");
                    if token != manual {
                        continue;
                    }
                    ui.push_id(&definition.id, |ui| {
                        let stored = pack.credentials.configured.contains(&definition.id);
                        kit::row(
                            ui,
                            &definition.label,
                            if stored { "Stored" } else { "Not set" },
                            |ui| {
                                ui.horizontal_wrapped(|ui| {
                                    let mut input = egui::TextEdit::singleline(
                                        draft.values.entry(definition.id.clone()).or_default(),
                                    )
                                    .password(true)
                                    .desired_width(ui.available_width().min(180.0))
                                    .hint_text("Enter new value")
                                    .show(ui);
                                    // Password masking alone retains plaintext undo
                                    // history. Do not let it restore submitted drafts.
                                    input.state.clear_undoer();
                                    input.state.store(ui.ctx(), input.response.id);
                                    if ui
                                        .add_enabled(
                                            stored,
                                            egui::Button::new("Clear saved").small(),
                                        )
                                        .clicked()
                                    {
                                        clear = Some(definition.id.clone());
                                    }
                                });
                            },
                        );
                    });
                }
            };
            fields(ui, false);
            if twitch {
                egui::CollapsingHeader::new("Manual tokens")
                    .default_open(false)
                    .show(ui, |ui| {
                        kit::note(
                            ui,
                            "Optional: enter existing tokens instead of browser sign-in.",
                        );
                        fields(ui, true);
                    });
            }
            save = ui
                .add_enabled(
                    draft.values.values().any(|value| !value.is_empty()),
                    egui::Button::new("Save credentials"),
                )
                .clicked();
        });
    });
    if let Some(id) = clear {
        // This delta clears only the selected saved field, never other IDs.
        draft.values.clear();
        extensions.set_credentials(pack.installed.installation_id, BTreeMap::from([(id, None)]));
    } else if save {
        let updates = draft.take_updates();
        extensions.set_credentials(pack.installed.installation_id, updates);
    } else if draft.values.values().any(|value| !value.is_empty()) {
        ui.ctx().data_mut(|data| data.insert_temp(draft_id, draft));
    }
    if capture {
        section.header_response.scroll_to_me(Some(egui::Align::TOP));
    }
}

#[derive(Clone)]
struct SettingsDraft {
    revision: uuid::Uuid,
    saved: Settings,
    values: Settings,
}

fn default_setting(kind: &SettingKind) -> SettingValue {
    match kind {
        SettingKind::Toggle { default } => SettingValue::Bool(*default),
        SettingKind::Integer { default, .. } => SettingValue::Integer(*default),
        SettingKind::Text { default, .. }
        | SettingKind::Choice { default, .. }
        | SettingKind::Motion { default }
        | SettingKind::Expression { default }
        | SettingKind::Character { default } => SettingValue::Text(default.clone()),
    }
}

fn default_settings(definitions: &[SettingDefinition]) -> Settings {
    definitions
        .iter()
        .map(|definition| (definition.id.clone(), default_setting(&definition.kind)))
        .collect()
}

#[derive(Clone)]
struct CatalogChoice {
    id: String,
    label: String,
    search: String,
}

fn registered_motion_choices<'a>(
    packs: impl IntoIterator<Item = &'a ResolvedPack>,
) -> Vec<CatalogChoice> {
    let mut choices = BTreeMap::new();
    // Match ActionCatalog::from_packs: enabled resolved packs, declared motion,
    // matching local content identity, and an actual resolved asset path.
    for pack in packs {
        for motion in &pack.manifest.content.motions {
            if let Some(content) = pack.content.iter().find(|content| {
                content.kind == ContentKind::Motion && content.local_id == motion.id
            }) && content.path.is_some()
                && !content.id.is_empty()
                && content.id.len() <= MAX_REGISTERED_ID_BYTES
                && !content
                    .id
                    .chars()
                    .any(|c| c.is_whitespace() || c.is_control())
            {
                let label = format!("{} · {}", motion.name, pack.manifest.name);
                let search = format!("{label} {} {}", pack.manifest.id, content.id).to_lowercase();
                choices.insert(
                    content.id.clone(),
                    CatalogChoice {
                        id: content.id.clone(),
                        label,
                        search,
                    },
                );
            }
        }
    }
    let mut choices: Vec<_> = choices.into_values().collect();
    choices.sort_by_cached_key(|choice| (choice.label.to_lowercase(), choice.id.clone()));
    choices
}

fn available_expression_choices(available: &[String]) -> Vec<CatalogChoice> {
    let mut choices = BTreeMap::new();
    for name in available {
        if !name.trim().is_empty()
            && name.len() <= MAX_ID_BYTES
            && !name.chars().any(char::is_control)
            && !reserved_expression(name)
        {
            choices
                .entry(name.clone())
                .or_insert_with(|| CatalogChoice {
                    id: name.clone(),
                    label: name.clone(),
                    search: name.to_lowercase(),
                });
        }
    }
    let mut choices: Vec<_> = choices.into_values().collect();
    choices.sort_by_cached_key(|choice| (choice.search.clone(), choice.id.clone()));
    choices
}

fn available_character_choices(characters: &[waifu_studio::data::Character]) -> Vec<CatalogChoice> {
    let mut choices = BTreeMap::new();
    for character in characters {
        if character.id.is_nil() {
            continue;
        }
        let id = character.id.to_string();
        let label = if character.name.trim().is_empty() {
            "Unnamed character".to_owned()
        } else {
            character.name.clone()
        };
        choices.entry(id.clone()).or_insert_with(|| CatalogChoice {
            search: format!("{label} {id}").to_lowercase(),
            id,
            label,
        });
    }
    let mut choices: Vec<_> = choices.into_values().collect();
    choices.sort_by_cached_key(|choice| (choice.label.to_lowercase(), choice.id.clone()));
    choices
}

#[derive(Clone, Copy)]
enum CatalogKind {
    Motion,
    Expression,
    Character,
}

impl CatalogKind {
    fn salt(self) -> &'static str {
        match self {
            Self::Motion => "motion-setting",
            Self::Expression => "expression-setting",
            Self::Character => "character-setting",
        }
    }

    fn empty_label(self) -> &'static str {
        match self {
            Self::Motion | Self::Expression => "None",
            Self::Character => "Loaded character",
        }
    }
}

fn catalog_setting_label<'a>(
    current: &str,
    choices: &'a [CatalogChoice],
    kind: CatalogKind,
) -> &'a str {
    if current.is_empty() {
        kind.empty_label()
    } else {
        choices.iter().find(|choice| choice.id == current).map_or(
            match kind {
                CatalogKind::Character => "Unavailable character",
                _ => "Unavailable",
            },
            |choice| choice.label.as_str(),
        )
    }
}

// Search stays inside the selector and results use virtual rows, like the
// model catalog controls. Selecting returns registered IDs, never asset paths.
#[cfg(test)]
fn motion_setting_list(
    ui: &mut egui::Ui,
    current: &str,
    choices: &[CatalogChoice],
) -> Option<String> {
    catalog_setting_list(ui, current, choices, CatalogKind::Motion)
}

fn catalog_setting_list(
    ui: &mut egui::Ui,
    current: &str,
    choices: &[CatalogChoice],
    kind: CatalogKind,
) -> Option<String> {
    let search_id = ui.make_persistent_id(format!("{}-search", kind.salt()));
    let mut query = ui
        .ctx()
        .data(|data| data.get_temp::<String>(search_id))
        .unwrap_or_default();
    ui.add(
        egui::TextEdit::singleline(&mut query)
            .hint_text(match kind {
                CatalogKind::Motion => "Search motions or packs…",
                CatalogKind::Expression => "Search avatar expressions…",
                CatalogKind::Character => "Search characters or UUIDs…",
            })
            .desired_width(f32::INFINITY)
            .char_limit(128),
    );
    ui.ctx()
        .data_mut(|data| data.insert_temp(search_id, query.clone()));
    let words: Vec<_> = query.split_whitespace().map(str::to_lowercase).collect();
    let filtered: Vec<_> = choices
        .iter()
        .filter(|choice| words.iter().all(|word| choice.search.contains(word)))
        .collect();
    let mut picked = None;
    if ui
        .selectable_label(current.is_empty(), kind.empty_label())
        .clicked()
    {
        picked = Some(String::new());
    }
    ui.small(format!(
        "{} matching · {} available",
        filtered.len(),
        choices.len()
    ));
    egui::ScrollArea::vertical()
        .id_salt((kind.salt(), "rows"))
        .max_height(240.0)
        .show_rows(ui, 28.0, filtered.len(), |ui, rows| {
            for row in rows {
                let choice = filtered[row];
                if ui
                    .add_sized(
                        [ui.available_width(), 28.0],
                        egui::Button::selectable(current == choice.id, &choice.label).truncate(),
                    )
                    .on_hover_text(format!("{}\n{}", choice.label, choice.id))
                    .clicked()
                {
                    picked = Some(choice.id.clone());
                }
            }
        });
    if filtered.is_empty() {
        kit::note(
            ui,
            match kind {
                CatalogKind::Motion => "No matching motions in enabled packs.",
                CatalogKind::Expression => "No matching expressions on the current avatar.",
                CatalogKind::Character => "No matching characters in this library.",
            },
        );
    }
    picked
}

fn motion_setting_ui(ui: &mut egui::Ui, current: &mut String, choices: &[CatalogChoice]) {
    catalog_setting_ui(ui, current, choices, CatalogKind::Motion);
}

fn expression_setting_ui(ui: &mut egui::Ui, current: &mut String, choices: &[CatalogChoice]) {
    catalog_setting_ui(ui, current, choices, CatalogKind::Expression);
}

fn catalog_setting_ui(
    ui: &mut egui::Ui,
    current: &mut String,
    choices: &[CatalogChoice],
    kind: CatalogKind,
) {
    let width = ui.available_width().clamp(1.0, 220.0);
    ui.vertical(|ui| {
        ui.set_max_width(width);
        let label = catalog_setting_label(current, choices, kind);
        egui::ComboBox::from_id_salt(kind.salt()).width(width).height(360.0).truncate()
            .selected_text(label).show_ui(ui, |ui| {
                super::popup_layout(ui, 380.0);
                if let Some(picked) = catalog_setting_list(ui, current, choices, kind) {
                    *current = picked;
                    ui.close();
                }
            }).response.on_hover_text(label);
        if !current.is_empty() && !choices.iter().any(|choice| choice.id == *current) {
            let note = match kind {
                CatalogKind::Motion => "Selected motion is unavailable. Enable its pack or choose another motion. The selection is kept.".to_owned(),
                CatalogKind::Expression => "Selected expression is unavailable on the current avatar. Choose another expression or keep this selection.".to_owned(),
                CatalogKind::Character => format!("Selected character {current} is unavailable in this library. The saved UUID is kept. Choose another or use Loaded character."),
            };
            kit::note(ui, &note);
        }
    });
}

fn settings_ui(
    ui: &mut egui::Ui,
    extensions: &mut ExtensionState,
    pack: &PackView,
    motion_choices: &[CatalogChoice],
    expression_choices: &[CatalogChoice],
    character_choices: &[CatalogChoice],
) {
    let Some(content) = &pack.content else {
        return;
    };
    let definitions = &content.manifest.settings;
    let capture = ui
        .ctx()
        .data(|data| data.get_temp::<bool>(egui::Id::new("capture-advanced")))
        .unwrap_or(false);
    let section = egui::CollapsingHeader::new("Settings").default_open(true).show_unindented(ui, |ui| {
        let draft_id = ui.id().with("settings-draft");
        let mut draft = ui.ctx().data(|data| data.get_temp::<SettingsDraft>(draft_id))
            .filter(|draft| draft.revision == pack.installed.revision && draft.saved == pack.installed.settings)
            .unwrap_or_else(|| SettingsDraft {
                revision: pack.installed.revision,
                saved: pack.installed.settings.clone(),
                values: pack.installed.settings.clone(),
            });
        ui.add_enabled_ui(!extensions.busy && pack.error.is_none(), |ui| {
            for definition in definitions {
                ui.push_id(&definition.id, |ui| {
                    let value = draft.values.entry(definition.id.clone())
                        .or_insert_with(|| default_setting(&definition.kind));
                    kit::row(ui, &definition.label, &definition.description, |ui| {
                        match (&definition.kind, value) {
                            (SettingKind::Toggle { .. }, SettingValue::Bool(value)) => {
                                kit::toggle(ui, value);
                            }
                            (SettingKind::Integer { min, max, .. }, SettingValue::Integer(value)) => {
                                ui.spacing_mut().slider_width = (ui.available_width() - 72.0).clamp(48.0, 150.0);
                                ui.add(egui::Slider::new(value, *min..=*max).integer());
                            }
                            (SettingKind::Text { max_length, .. }, SettingValue::Text(value)) => {
                                ui.add(egui::TextEdit::singleline(value).desired_width(ui.available_width().min(200.0)).char_limit(*max_length))
                                    .on_hover_text(format!("Up to {max_length} UTF-8 bytes."));
                            }
                            (SettingKind::Choice { options, .. }, SettingValue::Text(value)) => {
                                egui::ComboBox::from_id_salt("value").width(ui.available_width().min(200.0)).truncate()
                                    .selected_text(value.as_str()).show_ui(ui, |ui| {
                                        super::popup_layout(ui, 340.0);
                                        for option in options {
                                            ui.selectable_value(value, option.clone(), option);
                                        }
                                    });
                            }
                            (SettingKind::Motion { .. }, SettingValue::Text(value)) => {
                                motion_setting_ui(ui, value, motion_choices);
                            }
                            (SettingKind::Expression { .. }, SettingValue::Text(value)) => {
                                expression_setting_ui(ui, value, expression_choices);
                            }
                            (SettingKind::Character { .. }, SettingValue::Text(value)) => {
                                catalog_setting_ui(ui, value, character_choices, CatalogKind::Character);
                            }
                            _ => { ui.colored_label(super::ACCENT, "Value does not match this setting. Reset defaults to repair the draft."); }
                        }
                    });
                });
            }
            let validation = validate_values(definitions, &draft.values);
            if let Err(error) = &validation {
                ui.colored_label(super::ACCENT, error);
            }
            ui.horizontal_wrapped(|ui| {
                if ui.add_enabled(draft.values != draft.saved && validation.is_ok(), egui::Button::new("Save settings").small()).clicked()
                    && let Ok(settings) = validation {
                    extensions.set_settings(pack.installed.installation_id, settings);
                }
                if ui.small_button("Reset defaults").clicked() {
                    draft.values = default_settings(definitions);
                }
            });
        });
        ui.ctx().data_mut(|data| data.insert_temp(draft_id, draft));
    });
    if capture {
        section.header_response.scroll_to_me(Some(egui::Align::TOP));
    }
}

fn remove_ui(ui: &mut egui::Ui, extensions: &mut ExtensionState, id: uuid::Uuid) {
    let confirm_id = ui.id().with("remove-confirm");
    let confirming = ui
        .ctx()
        .data(|d| d.get_temp::<bool>(confirm_id))
        .unwrap_or(false);
    if confirming {
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(!extensions.busy, egui::Button::new("Remove extension"))
                .clicked()
            {
                extensions.remove(id);
            }
            if ui.button("Keep extension").clicked() {
                ui.ctx().data_mut(|d| d.insert_temp(confirm_id, false));
            }
        });
    } else if ui.button("Remove…").clicked() {
        ui.ctx().data_mut(|d| d.insert_temp(confirm_id, true));
    }
}
fn content_label(pack: &ResolvedPack, id: &str) -> String {
    let Some(item) = pack.content.iter().find(|c| c.id == id) else {
        return "Choose content".into();
    };
    let c = &pack.manifest.content;
    let (kind, name) = match item.kind {
        ContentKind::Character => (
            "Character",
            c.characters
                .iter()
                .find(|c| c.id == item.local_id)
                .map(|c| c.name.as_str()),
        ),
        ContentKind::Motion => (
            "Motion",
            c.motions
                .iter()
                .find(|c| c.id == item.local_id)
                .map(|c| c.name.as_str()),
        ),
        ContentKind::Background => (
            "Background",
            c.backgrounds
                .iter()
                .find(|c| c.id == item.local_id)
                .map(|c| c.name.as_str()),
        ),
        ContentKind::Preset => (
            "Preset",
            c.presets
                .iter()
                .find(|c| c.id == item.local_id)
                .map(|c| c.name.as_str()),
        ),
        ContentKind::Expression => (
            "Expression",
            c.expressions
                .iter()
                .find(|c| c.id == item.local_id)
                .map(|c| c.name.as_str()),
        ),
    };
    format!("{kind} · {}", name.unwrap_or(&item.local_id))
}
fn apply(
    ctx: &egui::Context,
    studio: &mut Studio,
    animation: &mut AnimationState,
    pack: &ResolvedPack,
    id: &str,
) -> Result<(), String> {
    use waifu_studio::environments::EnvironmentKind as E;
    let item = pack
        .content
        .iter()
        .find(|c| c.id == id)
        .ok_or("Content is no longer available")?;
    let content = &pack.manifest.content;
    match item.kind {
        ContentKind::Character => {
            let entry = content
                .characters
                .iter()
                .find(|c| c.id == item.local_id)
                .ok_or("Character metadata is missing")?;
            let path = item.path.as_ref().ok_or("Character asset is missing")?;
            let existing = studio.library.characters.iter().any(|c| c.model == *path);
            studio.library.capture_selected_settings();
            let previous = studio.library.selected;
            let id = studio.library.import(path)?;
            if !existing
                && let Some(character) = studio.library.characters.iter_mut().find(|c| c.id == id)
            {
                character.name.clone_from(&entry.name);
                character.persona.clone_from(&entry.persona);
                character.greeting.clone_from(&entry.greeting);
            }
            if let Some(previous) = previous {
                studio.library.select_character(previous);
            } else {
                studio.library.selected = None;
            }
            studio.select_character(id);
        }
        ContentKind::Motion => {
            let path = item.path.as_ref().ok_or("Motion asset is missing")?;
            super::manual_motion(ctx, animation);
            animation.load_path(path)?;
        }
        ContentKind::Background => {
            let entry = content
                .backgrounds
                .iter()
                .find(|c| c.id == item.local_id)
                .ok_or("Background metadata is missing")?;
            match &entry.source {
                BackgroundSource::Image { .. } => {
                    let path = item.path.as_ref().ok_or("Background image is missing")?;
                    let (width, height) = image::image_dimensions(path)
                        .map_err(|e| format!("Cannot read background image: {e}"))?;
                    if width == 0
                        || height == 0
                        || u64::from(width) * u64::from(height) > 64 * 1024 * 1024
                    {
                        return Err("Background image dimensions are unsupported".into());
                    }
                    studio.library.scene.image = Some(path.clone());
                    studio.library.scene.environment = E::Disabled;
                }
                BackgroundSource::Room { room } => {
                    studio.library.scene.image = None;
                    studio.library.scene.environment = match room {
                        RoomId::CozyRoom => E::CozyRoom,
                        RoomId::MoonlitGarden => E::MoonlitGarden,
                        RoomId::NeonRooftop => E::NeonRooftop,
                        RoomId::RoseBedroom => E::RoseBedroom,
                        RoomId::VioletStudio => E::VioletStudio,
                        RoomId::CyanLounge => E::CyanLounge,
                    };
                }
            }
        }
        ContentKind::Preset => {
            let settings = &content
                .presets
                .iter()
                .find(|c| c.id == item.local_id)
                .ok_or("Preset metadata is missing")?
                .settings;
            let scene = &mut studio.library.scene;
            if let Some(v) = settings.background {
                scene.background = v;
            }
            if let Some(v) = settings.portrait {
                scene.portrait = v;
            }
            if let Some(v) = settings.idle_motion {
                scene.idle_motion = v;
            }
            if let Some(v) = settings.outlines {
                scene.outlines = v;
            }
            if let Some(v) = settings.room_view {
                scene.room_view = v;
            }
        }
        ContentKind::Expression => {
            return Err("Expression mappings follow reply emotion automatically".into());
        }
    }
    studio.save();
    studio.notice = Some(format!("Using {}", content_label(pack, id)));
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn running_process_does_not_imply_call_readiness() {
        assert!(
            super::call_output_guidance(true, true, true, Some(false), false).starts_with("Select")
        );

        assert!(
            super::call_output_guidance(true, true, true, Some(false), true).starts_with("Waiting")
        );
        assert!(
            super::call_output_guidance(true, true, true, Some(true), true).starts_with("Ready")
        );
        assert!(
            super::call_output_guidance(false, true, true, Some(true), true).starts_with("Enable")
        );
        assert!(
            super::call_output_guidance(true, false, true, Some(true), true).starts_with("Allow")
        );
        assert!(
            super::call_output_guidance(true, true, false, Some(true), true)
                .starts_with("Complete")
        );
    }

    use super::*;

    #[test]
    fn omitted_bundled_examples_stay_visible_but_cannot_be_clicked() {
        let root =
            std::env::temp_dir().join(format!("waifu-example-button-{}", uuid::Uuid::new_v4()));
        let ctx = egui::Context::default();
        let mut clicked = false;
        motion_frame(&ctx, vec![], |ui| {
            clicked |= bundled_example_button(ui, "Example", &root);
        });
        let output = motion_frame(&ctx, vec![], |ui| {
            clicked |= bundled_example_button(ui, "Example", &root);
        });
        let position = motion_text_rect(&output, "Example").center();
        motion_frame(&ctx, motion_click(position), |ui| {
            clicked |= bundled_example_button(ui, "Example", &root);
        });
        assert!(!clicked);
        std::fs::create_dir_all(&root).unwrap();
        // Availability is separate from validation at the installation boundary.
        std::fs::write(root.join("extension.json"), b"{}").unwrap();
        motion_frame(&ctx, vec![], |ui| {
            clicked |= bundled_example_button(ui, "Example", &root);
        });
        motion_frame(&ctx, motion_click(position), |ui| {
            clicked |= bundled_example_button(ui, "Example", &root);
        });
        assert!(clicked);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn saved_twitch_id_enables_connect_without_painting_values_or_accepting_invalid_override() {
        fn paints(shape: &egui::Shape, value: &str) -> bool {
            match shape {
                egui::Shape::Text(text) => text.galley.job.text.contains(value),
                egui::Shape::Vec(shapes) => shapes.iter().any(|shape| paints(shape, value)),
                _ => false,
            }
        }
        for (value, stored, available, expected) in [
            ("", true, true, true),
            ("", false, true, false),
            ("ReplacementId12", false, true, true),
            ("invalid replacement", true, true, false),
            ("", true, false, false),
        ] {
            let ctx = egui::Context::default();
            let mut rect = egui::Rect::NOTHING;
            let mut clicked = false;
            for _ in 0..2 {
                motion_frame(&ctx, vec![], |ui| {
                    let response = twitch_connect_control(ui, value, stored, available);
                    assert_eq!(response.enabled(), expected);
                    rect = response.rect;
                });
            }
            let output = motion_frame(&ctx, motion_click(rect.center()), |ui| {
                clicked |= twitch_connect_control(ui, value, stored, available).clicked();
            });
            assert_eq!(
                clicked, expected,
                "connect admission: {value:?}, stored={stored}"
            );
            if !value.is_empty() {
                assert!(
                    !output
                        .shapes
                        .iter()
                        .any(|shape| paints(&shape.shape, value))
                );
            }
            if stored && value.is_empty() {
                motion_text_rect(&output, "Uses your saved application ID");
            }
        }
    }

    #[test]
    fn twitch_manual_tokens_require_expansion_and_collapse_without_saving() {
        fn contains(output: &egui::FullOutput, needle: &str) -> bool {
            fn painted(shape: &egui::Shape, needle: &str) -> bool {
                match shape {
                    egui::Shape::Text(text) => text.galley.job.text.contains(needle),
                    egui::Shape::Vec(shapes) => shapes.iter().any(|shape| painted(shape, needle)),
                    _ => false,
                }
            }
            output
                .shapes
                .iter()
                .any(|shape| painted(&shape.shape, needle))
        }
        let root = std::env::temp_dir().join(format!("twitch-account-ui-{}", uuid::Uuid::new_v4()));
        let mut state = ExtensionState::new(root.clone());
        let wait = |state: &mut ExtensionState| {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while state.busy {
                state.poll();
                assert!(std::time::Instant::now() < deadline);
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            assert!(state.error.is_none(), "{:?}", state.error);
        };
        wait(&mut state);
        state.install_integration(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/extensions/twitch-live"),
        );
        wait(&mut state);
        let pack = state.packs[0].clone();
        assert!(!pack.installed.enabled);
        let ctx = egui::Context::default();
        ctx.style_mut_of(egui::Theme::Dark, |style| style.animation_time = 0.0);
        ctx.style_mut_of(egui::Theme::Light, |style| style.animation_time = 0.0);
        let mut frame =
            |events| motion_frame(&ctx, events, |ui| credentials_ui(ui, &mut state, &pack));
        frame(vec![]);
        let closed = frame(vec![]);
        assert!(!contains(&closed, "Connect with Twitch"));
        frame(motion_click(
            motion_text_rect(&closed, "Twitch account").center(),
        ));
        let account = frame(vec![]);
        assert!(contains(&account, "Connect with Twitch"));
        assert!(contains(&account, "Twitch application ID"));
        assert!(!contains(&account, "Twitch user access token"));
        assert!(!contains(&account, "Twitch refresh token"));
        frame(motion_click(
            motion_text_rect(&account, "Manual tokens").center(),
        ));
        let manual = frame(vec![]);
        assert!(contains(&manual, "Twitch user access token"));
        assert!(contains(&manual, "Twitch refresh token"));
        frame(motion_click(
            motion_text_rect(&manual, "Manual tokens").center(),
        ));
        let collapsed = frame(vec![]);
        assert!(!contains(&collapsed, "Twitch user access token"));
        assert!(!contains(&collapsed, "Twitch refresh token"));
        assert!(state.packs[0].credentials.configured.is_empty());
        assert!(!state.busy);
        assert!(
            state
                .twitch_auth_status(pack.installed.installation_id)
                .is_none()
        );
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn applying_first_pack_character_emits_selection_from_none_once() {
        let root = std::env::temp_dir().join(format!("pack-selection-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let mut json =
            br#"{"asset":{"version":"2.0"},"nodes":[],"scenes":[{"nodes":[]}],"scene":0}"#.to_vec();
        while !json.len().is_multiple_of(4) {
            json.push(b' ');
        }
        let mut glb = Vec::new();
        glb.extend_from_slice(b"glTF");
        glb.extend_from_slice(&2u32.to_le_bytes());
        glb.extend_from_slice(&(20u32 + json.len() as u32).to_le_bytes());
        glb.extend_from_slice(&(json.len() as u32).to_le_bytes());
        glb.extend_from_slice(b"JSON");
        glb.extend_from_slice(&json);
        std::fs::write(root.join("friend.glb"), glb).unwrap();
        let manifest = serde_json::json!({
            "api_version":1, "id":"org.example.selection-pack", "version":"1.0.0",
            "name":"Selection pack", "description":"Owned test fixture",
            "provenance":{"author":"Test", "license":"MIT"},
            "content":{"characters":[{"id":"friend", "name":"Friend", "asset":"friend.glb",
                "persona":"Friendly", "greeting":"Hello"}]},
        });
        std::fs::write(
            root.join("extension.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        let pack = waifu_studio::extension_manifest::load_pack(&root).unwrap();
        let (store, mut library) = waifu_studio::data::Store::open(root.join("profile")).unwrap();
        library.characters.clear();
        library.selected = None;
        let mut studio = Studio::new(store, library);
        let mut animation = AnimationState::default();
        let ctx = egui::Context::default();
        apply(
            &ctx,
            &mut studio,
            &mut animation,
            &pack,
            &pack.content[0].id,
        )
        .unwrap();
        let id = studio.library.selected.unwrap();
        let events = studio.lifecycle.drain();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].event,
            waifu_studio::extension_lifecycle::LifecycleEvent::CharacterSelected {
                previous: None,
                new: Some(id),
                selection_revision: 1,
            }
        );
        apply(
            &ctx,
            &mut studio,
            &mut animation,
            &pack,
            &pack.content[0].id,
        )
        .unwrap();
        assert!(
            studio.lifecycle.drain().is_empty(),
            "same selected pack character emitted again"
        );
        assert_eq!(hook_event_label("character.selected"), "Character selected");
        assert_eq!(hook_event_label("emotion.changed"), "Emotion changed");
        drop(studio);
        std::fs::remove_dir_all(root).unwrap();
    }

    fn catalog_character(id: uuid::Uuid, name: &str) -> waifu_studio::data::Character {
        serde_json::from_value(serde_json::json!({
            "id": id, "name": name, "model": "unused.vrm", "persona": "", "format": "VRM",
        }))
        .unwrap()
    }

    #[test]
    fn character_catalog_uses_library_identity_and_names_with_stable_defaults() {
        let alice = uuid::Uuid::new_v4();
        let zoe = uuid::Uuid::new_v4();
        let choices = available_character_choices(&[
            catalog_character(zoe, "Zoe"),
            catalog_character(alice, "Alice"),
            catalog_character(alice, "Duplicate"),
            catalog_character(uuid::Uuid::nil(), "Invalid identity"),
        ]);
        assert_eq!(choices.len(), 2);
        assert_eq!(choices[0].label, "Alice");
        assert_eq!(choices[0].id, alice.to_string());
        assert!(choices[0].search.contains(&alice.to_string()));
        assert_eq!(choices[1].id, zoe.to_string());
        assert!(available_character_choices(&[]).is_empty());
        assert_eq!(
            default_setting(&SettingKind::Character {
                default: String::new()
            }),
            SettingValue::Text(String::new())
        );
        assert_eq!(
            default_setting(&SettingKind::Character {
                default: alice.to_string()
            }),
            SettingValue::Text(alice.to_string())
        );
        assert_eq!(
            catalog_setting_label("", &choices, CatalogKind::Character),
            "Loaded character"
        );
        assert_eq!(
            catalog_setting_label("", &choices, CatalogKind::Motion),
            "None"
        );
        assert_eq!(
            catalog_setting_label("", &choices, CatalogKind::Expression),
            "None"
        );
    }

    #[test]
    fn character_selector_retains_missing_uuid_and_selects_character_or_loaded_character() {
        let ctx = egui::Context::default();
        let selected = uuid::Uuid::new_v4();
        let choices = available_character_choices(&[catalog_character(selected, "Alice")]);
        let missing = uuid::Uuid::new_v4().to_string();
        let mut value = missing.clone();
        {
            let mut frame = |events| {
                motion_frame(&ctx, events, |ui| {
                    catalog_setting_ui(ui, &mut value, &choices, CatalogKind::Character);
                })
            };
            frame(vec![]);
            let closed = frame(vec![]);
            motion_text_rect(&closed, &missing);
            motion_text_rect(&closed, "saved UUID is kept");
        }
        assert_eq!(value, missing);
        {
            let mut frame = |events| {
                motion_frame(&ctx, events, |ui| {
                    catalog_setting_ui(ui, &mut value, &choices, CatalogKind::Character);
                })
            };
            let closed = frame(vec![]);
            frame(motion_click(
                motion_text_rect(&closed, "Unavailable character").center(),
            ));
            let open = frame(vec![]);
            frame(motion_click(motion_text_rect(&open, "Alice").center()));
        }
        assert_eq!(value, selected.to_string());
        {
            let mut frame = |events| {
                motion_frame(&ctx, events, |ui| {
                    catalog_setting_ui(ui, &mut value, &choices, CatalogKind::Character);
                })
            };
            let closed = frame(vec![]);
            frame(motion_click(motion_text_rect(&closed, "Alice").center()));
            let open = frame(vec![]);
            frame(motion_click(
                motion_text_rect(&open, "Loaded character").center(),
            ));
        }
        assert!(value.is_empty());
    }

    #[test]
    fn character_search_matches_name_and_uuid_and_returns_saved_identity() {
        let alice = uuid::Uuid::new_v4();
        let choices = available_character_choices(&[
            catalog_character(uuid::Uuid::new_v4(), "Alice Other"),
            catalog_character(alice, "Alice Desired"),
        ]);
        let ctx = egui::Context::default();
        let mut picked = None;
        {
            let mut frame = |events| {
                motion_frame(&ctx, events, |ui| {
                    let search = ui.make_persistent_id("character-setting-search");
                    ui.ctx()
                        .data_mut(|data| data.insert_temp(search, format!("ALICE  {alice}")));
                    if let Some(value) =
                        catalog_setting_list(ui, "", &choices, CatalogKind::Character)
                    {
                        picked = Some(value);
                    }
                })
            };
            frame(vec![]);
            let output = frame(vec![]);
            motion_text_rect(&output, "1 matching · 2 available");
            frame(motion_click(
                motion_text_rect(&output, "Alice Desired").center(),
            ));
        }
        assert_eq!(picked, Some(alice.to_string()));
    }

    #[test]
    fn expression_choices_exclude_reserved_controls_and_byte_overflow_but_allow_spaces() {
        let at_limit = format!("{}xy", "雪".repeat(42));
        assert_eq!(at_limit.len(), MAX_ID_BYTES);
        let available = vec![
            "Soft smile".into(),
            "Soft smile".into(),
            at_limit.clone(),
            format!("{at_limit}z"),
            " ".into(),
            String::new(),
            "bad\nname".into(),
            "bad\u{7f}name".into(),
            "BLINKLeft".into(),
            "lookRight".into(),
            "mouthSmile".into(),
            "aa".into(),
            "I".into(),
        ];
        let choices = available_expression_choices(&available);
        assert_eq!(choices.len(), 2);
        assert!(choices.iter().any(|choice| choice.id == "Soft smile"));
        assert!(choices.iter().any(|choice| choice.id == at_limit));
        assert!(
            choices
                .iter()
                .all(|choice| !reserved_expression(&choice.id))
        );
        assert!(available_expression_choices(&[]).is_empty());
        assert_eq!(
            default_setting(&SettingKind::Expression {
                default: "Soft smile".into()
            }),
            SettingValue::Text("Soft smile".into())
        );
        assert_eq!(
            default_setting(&SettingKind::Expression {
                default: String::new()
            }),
            SettingValue::Text(String::new())
        );
    }

    #[test]
    fn expression_selector_retains_unavailable_then_selects_name_and_none() {
        let ctx = egui::Context::default();
        let choices = available_expression_choices(&["Soft smile".into(), "Happy".into()]);
        let mut value = "Former expression".to_owned();
        for _ in 0..2 {
            motion_frame(&ctx, vec![], |ui| {
                expression_setting_ui(ui, &mut value, &[])
            });
        }
        assert_eq!(value, "Former expression");
        {
            let mut frame = |events| {
                motion_frame(&ctx, events, |ui| {
                    expression_setting_ui(ui, &mut value, &choices)
                })
            };
            let closed = frame(vec![]);
            motion_text_rect(&closed, "keep this selection");
            frame(motion_click(
                motion_text_rect(&closed, "Unavailable").center(),
            ));
            let open = frame(vec![]);
            frame(motion_click(motion_text_rect(&open, "Soft smile").center()));
        }
        assert_eq!(value, "Soft smile");
        {
            let mut frame = |events| {
                motion_frame(&ctx, events, |ui| {
                    expression_setting_ui(ui, &mut value, &choices)
                })
            };
            let closed = frame(vec![]);
            frame(motion_click(
                motion_text_rect(&closed, "Soft smile").center(),
            ));
            let open = frame(vec![]);
            frame(motion_click(motion_text_rect(&open, "None").center()));
        }
        assert!(value.is_empty());
    }

    #[test]
    fn expression_search_matches_words_without_case_or_spacing_restrictions() {
        let choices = available_expression_choices(&[
            "Soft smile".into(),
            "Angry".into(),
            "Soft wink".into(),
        ]);
        let ctx = egui::Context::default();
        let mut selected = None;
        {
            let mut frame = |events| {
                motion_frame(&ctx, events, |ui| {
                    let search = ui.make_persistent_id("expression-setting-search");
                    ui.ctx()
                        .data_mut(|data| data.insert_temp(search, "SMILE  soft".to_owned()));
                    if let Some(value) =
                        catalog_setting_list(ui, "", &choices, CatalogKind::Expression)
                    {
                        selected = Some(value);
                    }
                })
            };
            frame(vec![]);
            let output = frame(vec![]);
            motion_text_rect(&output, "1 matching · 3 available");
            frame(motion_click(
                motion_text_rect(&output, "Soft smile").center(),
            ));
        }
        assert_eq!(selected.as_deref(), Some("Soft smile"));
    }

    #[test]
    fn long_expression_names_do_not_widen_selector_or_change_the_value() {
        let name = "W".repeat(MAX_ID_BYTES);
        let choices = available_expression_choices(std::slice::from_ref(&name));
        let ctx = egui::Context::default();
        let mut value = name.clone();
        let mut rect = egui::Rect::NOTHING;
        for _ in 0..2 {
            motion_frame(&ctx, vec![], |ui| {
                ui.set_max_width(240.0);
                rect = ui
                    .vertical(|ui| expression_setting_ui(ui, &mut value, &choices))
                    .response
                    .rect;
            });
        }
        assert!(
            rect.width() <= 240.0,
            "expression widened control: {rect:?}"
        );
        assert_eq!(value, name);
    }

    fn motion_frame(
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        add: impl FnMut(&mut egui::Ui),
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
            add,
        );
        output.textures_delta.clear();
        output
    }

    fn motion_text_rect(output: &egui::FullOutput, needle: &str) -> egui::Rect {
        fn find(shape: &egui::Shape, needle: &str) -> Option<egui::Rect> {
            match shape {
                egui::Shape::Text(text) if text.galley.job.text.contains(needle) => {
                    Some(egui::Rect::from_min_size(text.pos, text.galley.size()))
                }
                egui::Shape::Vec(shapes) => shapes.iter().find_map(|shape| find(shape, needle)),
                _ => None,
            }
        }
        output
            .shapes
            .iter()
            .find_map(|shape| find(&shape.shape, needle))
            .unwrap_or_else(|| panic!("Missing motion selector text: {needle}"))
    }

    fn motion_click(pos: egui::Pos2) -> Vec<egui::Event> {
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
    fn motion_selector_selects_registered_id_retains_unavailable_and_allows_none() {
        let choices = [CatalogChoice {
            id: "org.example.starter/nod".into(),
            label: "Gentle nod · Starter".into(),
            search: "gentle nod starter".into(),
        }];
        let ctx = egui::Context::default();
        let mut current = "org.example.disabled/old".to_owned();
        {
            let mut frame = |events, choices: &[CatalogChoice]| {
                motion_frame(&ctx, events, |ui| {
                    motion_setting_ui(ui, &mut current, choices)
                })
            };
            frame(vec![], &choices);
            let closed = frame(vec![], &choices);
            let pos = motion_text_rect(&closed, "Unavailable").center();
            frame(motion_click(pos), &choices);
            let open = frame(vec![], &choices);
            let pos = motion_text_rect(&open, "Gentle nod · Starter").center();
            frame(motion_click(pos), &choices);
        }
        assert_eq!(current, choices[0].id);

        let pos = {
            let mut frame =
                |events| motion_frame(&ctx, events, |ui| motion_setting_ui(ui, &mut current, &[]));
            let unavailable = frame(vec![]);
            motion_text_rect(&unavailable, "selection is kept");
            let pos = motion_text_rect(&unavailable, "Unavailable").center();
            frame(motion_click(pos));
            let open = frame(vec![]);
            motion_text_rect(&open, "None").center()
        };
        assert_eq!(current, choices[0].id, "losing the pack must retain its ID");
        motion_frame(&ctx, motion_click(pos), |ui| {
            motion_setting_ui(ui, &mut current, &[])
        });
        assert!(current.is_empty());
    }

    #[test]
    fn motion_search_reaches_late_entries_by_package_name_with_virtual_rows() {
        let choices: Vec<_> = (0..500)
            .map(|index| CatalogChoice {
                id: format!("org.example.pack-{index:04}/nod"),
                label: format!("Gentle nod · Pack-{index:04}"),
                search: format!("gentle nod pack-{index:04}"),
            })
            .collect();
        let ctx = egui::Context::default();
        let mut picked = None;
        {
            let mut frame = |events| {
                motion_frame(&ctx, events, |ui| {
                    let search = ui.make_persistent_id("motion-setting-search");
                    ui.ctx()
                        .data_mut(|data| data.insert_temp(search, "pack-0499".to_owned()));
                    if let Some(value) = motion_setting_list(ui, "", &choices) {
                        picked = Some(value);
                    }
                })
            };
            frame(vec![]);
            let output = frame(vec![]);
            motion_text_rect(&output, "1 matching · 500 available");
            let pos = motion_text_rect(&output, "Gentle nod · Pack-0499").center();
            frame(motion_click(pos));
        }
        assert_eq!(picked.as_deref(), Some("org.example.pack-0499/nod"));
    }

    #[test]
    fn motion_choices_follow_resolved_catalog_identity_and_protocol_byte_bounds() {
        use waifu_studio::extension_protocol::{Action, PlaybackMode};
        let mut pack = waifu_studio::extension_manifest::load_pack(
            &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/extensions/companion-starter"),
        )
        .unwrap();
        pack.manifest.content.motions.truncate(1);
        let local = pack.manifest.content.motions[0].id.clone();
        pack.content
            .retain(|content| content.kind == ContentKind::Motion && content.local_id == local);
        assert_eq!(pack.content.len(), 1);
        let at_limit = format!(
            "org.example.{}/nod",
            "a".repeat(MAX_REGISTERED_ID_BYTES - "org.example./nod".len())
        );
        for id in [&at_limit, "org.example.pack/nod"] {
            pack.content[0].id = id.to_owned();
            let choices = registered_motion_choices([&pack]);
            assert_eq!(choices.len(), 1);
            assert_eq!(&choices[0].id, id);
            assert!(choices[0].label.contains(&pack.manifest.name));
            assert!(
                Action::MotionPlay {
                    motion_id: choices[0].id.clone(),
                    mode: PlaybackMode::OneShot
                }
                .validate()
                .is_ok()
            );
        }
        for id in [
            format!("{at_limit}x"),
            "org.example.pack/with space".into(),
            "org.example.pack/\u{2003}".into(),
            "org.example.pack/\u{7f}".into(),
            String::new(),
        ] {
            pack.content[0].id = id;
            assert!(registered_motion_choices([&pack]).is_empty());
        }
        pack.content[0].id = "org.example.pack/nod".into();
        pack.content[0].path = None;
        assert!(registered_motion_choices([&pack]).is_empty());
        assert!(registered_motion_choices(std::iter::empty()).is_empty());
    }

    #[test]
    fn motion_default_is_a_text_id_and_long_labels_stay_within_control_width() {
        let original = "org.example.disabled/nod".to_owned();
        assert_eq!(
            default_setting(&SettingKind::Motion {
                default: original.clone()
            }),
            SettingValue::Text(original)
        );
        let choices = [CatalogChoice {
            id: "org.example.pack/nod".into(),
            label: "Long motion and package name ".repeat(40),
            search: "long".into(),
        }];
        let ctx = egui::Context::default();
        let mut value = choices[0].id.clone();
        let mut rect = egui::Rect::NOTHING;
        for _ in 0..2 {
            motion_frame(&ctx, vec![], |ui| {
                ui.set_max_width(240.0);
                rect = ui
                    .vertical(|ui| motion_setting_ui(ui, &mut value, &choices))
                    .response
                    .rect;
            });
        }
        assert!(
            rect.width() <= 240.0,
            "motion name widened control: {rect:?}"
        );
        assert_eq!(value, choices[0].id);
    }

    #[test]
    fn credential_save_moves_only_nonempty_drafts_and_clears_entries() {
        let mut draft = CredentialsDraft {
            values: BTreeMap::from([
                ("token".into(), "test-only-token".into()),
                ("untouched".into(), String::new()),
                ("erased-draft".into(), String::new()),
            ]),
            ..Default::default()
        };
        let updates = draft.take_updates();
        assert_eq!(updates.len(), 1);
        assert_eq!(
            updates.get("token").and_then(Option::as_deref),
            Some("test-only-token")
        );
        assert!(!updates.contains_key("untouched"));
        assert!(!updates.contains_key("erased-draft"));
        assert!(draft.values.is_empty());
        assert!(draft.take_updates().is_empty());
    }

    fn settled(ctx: &egui::Context, state: &mut ExtensionState, animation: &mut AnimationState) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            poll(ctx, state, animation);
            if !state.busy {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "extension worker timed out"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(state.error.is_none(), "{:?}", state.error);
    }
    #[test]
    fn installed_catalog_survives_restart_and_disable_revokes_manual_pose_without_releasing_hold() {
        let root = std::env::temp_dir().join(format!("extension-ui-{}", uuid::Uuid::new_v4()));
        let ctx = egui::Context::default();
        let mut animation = AnimationState::default();
        let mut state = ExtensionState::new(root.clone());
        settled(&ctx, &mut state, &mut animation);
        state.install(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/extensions/companion-starter"),
        );
        settled(&ctx, &mut state, &mut animation);
        let entries = motions(&ctx);
        assert_eq!(entries.len(), 3);
        assert_eq!(
            super::super::companion_motion::playlist(&entries, &[], true).len(),
            2
        );
        let id = state.packs[0].installed.installation_id;
        let path = entries[0].path.clone();
        let clip =
            waifu_studio::animation::parse_bvh(&std::fs::read_to_string(&path).unwrap()).unwrap();
        animation.select_clip(Arc::new(clip), Some(path));
        animation.take_manual_control();
        lock(&companion_shared(&ctx)).manual_override();
        state.set_enabled(id, false);
        settled(&ctx, &mut state, &mut animation);
        assert!(motions(&ctx).is_empty());
        assert!(!animation.is_posing());
        assert!(animation.manual_controlled());
        assert!(lock(&companion_shared(&ctx)).paused);
        // Imported files stay available, even though automatic pack recall is revoked.
        assert!(entries[0].path.is_file());
        drop(state);
        let fresh = egui::Context::default();
        let mut state = ExtensionState::new(root.clone());
        settled(&fresh, &mut state, &mut animation);
        assert_eq!(state.packs[0].installed.installation_id, id);
        assert!(!state.packs[0].installed.enabled);
        assert!(motions(&fresh).is_empty());
        state.set_enabled(id, true);
        settled(&fresh, &mut state, &mut animation);
        assert_eq!(motions(&fresh).len(), 3);
        assert!(
            animation.manual_controlled(),
            "enabling content cannot release a manual hold"
        );
        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }
    #[test]
    fn authored_roles_and_emotions_work_with_opaque_names_and_loop_restrictions() {
        use waifu_studio::extension_manifest::{Emotion, MotionRole};
        let root = std::env::temp_dir().join(format!("extension-roles-{}", uuid::Uuid::new_v4()));
        let ctx = egui::Context::default();
        let mut animation = AnimationState::default();
        let mut state = ExtensionState::new(root.clone());
        settled(&ctx, &mut state, &mut animation);
        state.install(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/extensions/companion-starter"),
        );
        settled(&ctx, &mut state, &mut animation);
        let mut entries = motions(&ctx).as_ref().clone();
        drop(state);
        let _ = std::fs::remove_dir_all(root);
        for (index, entry) in entries.iter_mut().enumerate() {
            entry.path = format!("/pack/{index}.bvh").into();
        }
        assert_eq!(
            entries[1].category(),
            motion_library::MotionCategory::Talking
        );
        assert_eq!(
            super::super::companion_motion::playlist(&entries, &[], true).len(),
            2
        );
        entries[1].metadata.as_mut().unwrap().loopable = Some(false);
        assert_eq!(
            super::super::companion_motion::playlist(&entries, &[], true).len(),
            1
        );
        let metadata = entries[0].metadata.as_mut().unwrap();
        metadata.role = MotionRole::Gesture;
        metadata.emotion = Some(Emotion::Happy);
        let matched = super::super::motion_decision::matching_motions(
            waifu_studio::reply_affect::AffectEmotion::Happy,
            &entries,
        );
        assert_eq!(matched, vec![entries[0].path.clone()]);
        let offered = motion_library::MotionCatalog::offer(
            &entries,
            &[motion_library::MotionCategory::Talking],
            3,
        );
        entries[2].metadata.as_mut().unwrap().role = MotionRole::Idle;
        assert!(
            !offered.is_current(&entries),
            "a role edit invalidates outstanding decisions"
        );
    }
}
