//! Studio chrome: conversation overlays the avatar; settings occupy an opaque full page.
// UI-only catalogs and page adapters live beside this file.
#[path = "extension_page.rs"]
mod extension_page;
#[path = "memory_page.rs"]
mod memory_page;
#[path = "personality_catalog.rs"]
pub(crate) mod personality_catalog;
#[path = "settings_pages.rs"]
mod settings_pages;

#[cfg(test)]
use crate::motion_services::companion_motion;
use crate::motion_services::{
    self, MotionServices, SUGGESTED_CATEGORIES, SharedCompanion, SharedReplyMotion,
    SharedSuggester, motion_decision, motion_library,
};
use crate::studio::{Studio, Tab, ViewMode};
use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use egui::{Color32, RichText, Stroke, Vec2 as EVec2};
use waifu_studio::settings_kit::{self, self as kit};
use waifu_studio::{
    animation::AnimationState,
    loading::{AvatarLoadState, LoadStatus},
    voice::VoiceAdapter,
};

const TEXT: Color32 = Color32::from_rgb(232, 232, 232);
const MUTED: Color32 = Color32::from_rgb(147, 147, 147);
const ACCENT: Color32 = Color32::from_rgb(202, 112, 112);
const LINE: Color32 = Color32::from_rgb(46, 46, 46);
const SURFACE: Color32 = Color32::from_rgb(23, 23, 23);

pub fn style(ctx: &egui::Context) {
    let mut s = (*ctx.style_of(egui::Theme::Dark)).clone();
    s.visuals = egui::Visuals::dark();
    s.visuals.override_text_color = Some(TEXT);
    s.visuals.panel_fill = SURFACE;
    s.visuals.window_fill = Color32::from_rgb(29, 29, 29);
    s.visuals.window_stroke = Stroke::new(1.0, LINE);
    s.visuals.window_corner_radius = egui::CornerRadius::same(10);
    s.visuals.menu_corner_radius = egui::CornerRadius::same(8);
    s.visuals.extreme_bg_color = Color32::from_rgb(26, 26, 26);
    s.visuals.faint_bg_color = Color32::from_rgb(43, 43, 43);
    s.visuals.selection.bg_fill = Color32::from_rgb(85, 54, 52);
    s.visuals.selection.stroke = Stroke::new(1.0, TEXT);
    s.visuals.widgets.inactive.bg_fill = Color32::from_rgb(41, 41, 41);
    s.visuals.widgets.inactive.weak_bg_fill = Color32::from_rgb(41, 41, 41);
    s.visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, LINE);
    s.visuals.widgets.hovered.bg_fill = Color32::from_rgb(53, 53, 53);
    s.visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(53, 53, 53);
    s.visuals.widgets.active.bg_fill = Color32::from_rgb(84, 55, 54);
    s.spacing.item_spacing = EVec2::new(8.0, 6.0);
    s.spacing.button_padding = EVec2::new(8.0, 4.0);
    s.spacing.interact_size.y = 26.0;
    s.spacing.combo_width = 200.0;
    s.spacing.text_edit_width = 220.0;
    s.spacing.slider_width = 150.0;
    s.spacing.menu_width = 280.0;
    s.wrap_mode = Some(egui::TextWrapMode::Wrap);
    s.text_styles
        .insert(egui::TextStyle::Body, egui::FontId::proportional(14.0));
    s.text_styles
        .insert(egui::TextStyle::Button, egui::FontId::proportional(14.0));
    s.text_styles
        .insert(egui::TextStyle::Small, egui::FontId::proportional(12.0));
    s.text_styles
        .insert(egui::TextStyle::Heading, egui::FontId::proportional(19.0));
    ctx.set_style_of(egui::Theme::Dark, s);
    ctx.set_theme(egui::Theme::Dark);
}

/// Apply the shared chrome before either Studio or Desktop draws its menus.
pub(crate) fn ensure_style(ctx: &egui::Context) {
    let id = egui::Id::new("studio_styled");
    if !ctx.data(|data| data.get_temp::<bool>(id).unwrap_or(false)) {
        style(ctx);
        ctx.data_mut(|data| data.insert_temp(id, true));
    }
}

#[derive(bevy::ecs::system::SystemParam)]
pub struct AvatarUi<'w> {
    motions: Res<'w, MotionServices>,
    extensions: ResMut<'w, waifu_studio::extensions::ExtensionState>,
    extension_runtime: Option<Res<'w, waifu_studio::extension_runtime::ExtensionRuntime>>,
    speech_outputs: Option<Res<'w, waifu_studio::extension_speech_output::SpeechOutputs>>,
    capture: Option<Res<'w, crate::offscreen::OffscreenCapture>>,
    gaze: ResMut<'w, waifu_studio::gaze::GazeState>,
    preview: ResMut<'w, crate::avatar_preview::AvatarPreview>,
    catalogs: ResMut<'w, waifu_studio::model_catalog::SpeechCatalogs>,
    scene_chat: ResMut<'w, waifu_studio::scene_chat::SceneChat>,
    roster: ResMut<'w, waifu_studio::scene_roster::SceneRoster>,
    voice_library: ResMut<'w, waifu_studio::voice_library::VoiceLibraryState>,
    visemes: ResMut<'w, waifu_studio::viseme_audio::VisemeState>,
    facial: ResMut<'w, waifu_studio::expressions::FacialState>,
    appearance: ResMut<'w, waifu_studio::appearance::AppearanceState>,
    behavior: ResMut<'w, waifu_studio::behavior::BehaviorState>,
    physics: ResMut<'w, waifu_studio::spring_physics::SpringPhysicsState>,
    hotkeys: Option<ResMut<'w, crate::voice_hotkeys::VoiceHotkeys>>,
}

fn ready_call_outputs(
    outputs: Option<&waifu_studio::extension_speech_output::SpeechOutputs>,
    runtime: Option<&waifu_studio::extension_runtime::ExtensionRuntime>,
    character: Option<uuid::Uuid>,
) -> Option<Vec<uuid::Uuid>> {
    let (outputs, runtime) = (outputs?, runtime?);
    Some(
        character
            .map(|character| {
                outputs
                    .current_ready_routes_for_character(runtime, character)
                    .into_iter()
                    .map(|route| route.installation_id)
                    .collect()
            })
            .unwrap_or_default(),
    )
}

#[allow(clippy::too_many_arguments)] // Bevy injects each independent resource.
pub fn studio_ui(
    mut ws: ResMut<waifu_studio::fish_ws::FishWsState>,
    mut fish: ResMut<waifu_studio::fish_live::FishLiveState>,
    mut avatar_ui: AvatarUi,
    mut semantic: ResMut<waifu_studio::semantic_memory::SemanticMemory>,
    mut board: ResMut<waifu_studio::task_board::TaskBoard>,
    mut contexts: EguiContexts,
    mut studio: ResMut<Studio>,
    load: Res<AvatarLoadState>,
    mut desktop: ResMut<crate::desktop_settings::DesktopSettings>,
    mut animation: ResMut<AnimationState>,
    mut listening: ResMut<waifu_studio::listen::ListeningState>,
    mut memory: ResMut<waifu_studio::memory::MemoryState>,
    mut mcp: ResMut<waifu_studio::mcp::McpState>,
    mut agents: ResMut<waifu_studio::agents::AgentState>,
    mut orchestration: ResMut<crate::orchestration::OrchestrationState>,
    mut voice_design: ResMut<waifu_studio::voice_design::VoiceDesignState>,
) {
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    ensure_style(ctx);
    bind_motion_services(ctx, &avatar_ui.motions);
    if lock(&avatar_ui.motions.suggester).busy() {
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
    if avatar_ui.capture.as_ref().is_some_and(|capture| capture.1) {
        ctx.data_mut(|data| data.insert_temp(egui::Id::new("capture-advanced"), true));
    }
    if matches!(studio.mode, ViewMode::Desktop) {
        return;
    }
    let screen = ctx.content_rect();
    let narrow = screen.width() < 900.0;
    let rail = if !matches!(studio.tab, Tab::Chat) && screen.width() >= 800.0 {
        184.0
    } else {
        60.0
    };
    studio.ensure_models();
    let left = rail + if narrow { 24.0 } else { 44.0 };
    let name = studio
        .library
        .selected_character()
        .map(|c| c.name.clone())
        .unwrap_or_else(|| "Your companion".into());

    if studio.library.scene.image.is_some()
        || studio.library.scene.environment != waifu_studio::environments::EnvironmentKind::Disabled
    {
        // Keep headings legible over bright user images without intercepting input.
        let mut shade = egui::Mesh::default();
        for (position, alpha) in [
            (egui::pos2(rail, 0.0), 220),
            (egui::pos2(screen.right(), 0.0), 220),
            (egui::pos2(screen.right(), 330.0), 0),
            (egui::pos2(rail, 330.0), 0),
        ] {
            shade.colored_vertex(position, Color32::from_black_alpha(alpha));
        }
        shade.add_triangle(0, 1, 2);
        shade.add_triangle(0, 2, 3);
        ctx.layer_painter(egui::LayerId::background()).add(shade);
    }

    let mut viewport_ui = egui::Ui::new(
        ctx.clone(),
        "studio_viewport".into(),
        egui::UiBuilder::new()
            .layer_id(egui::LayerId::background())
            .max_rect(screen),
    );
    egui::Panel::left("navigation")
        .exact_size(rail)
        .resizable(false)
        .frame(
            egui::Frame::new()
                .fill(SURFACE)
                .stroke(Stroke::new(1.0, LINE))
                .inner_margin(8),
        )
        .show(&mut viewport_ui, |ui| {
            ui.add_space(18.0);
            egui::ScrollArea::vertical()
                .id_salt("navigation_scroll")
                .max_height((screen.height() - 70.0).max(100.0))
                .show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.label(RichText::new("W.").size(24.0).strong().color(TEXT));
                        ui.add_space(18.0);
                        for (tab, label, icon) in [
                            (Tab::Chat, "Conversation", 0),
                            (Tab::SceneChat, "Group chat", 0),
                            (Tab::Characters, "Profiles", 1),
                            (Tab::Scene, "Scene & avatar", 2),
                            (Tab::Connections, "Connections", 3),
                            (Tab::Voice, "Voice & mic", 6),
                            (Tab::Memory, "Memory", 7),
                            (Tab::Tools, "Assistant tools", 8),
                            (Tab::Tasks, "Agent tasks", 9),
                            (Tab::Extensions, "Extensions", 8),
                        ] {
                            let selected =
                                std::mem::discriminant(&studio.tab) == std::mem::discriminant(&tab);
                            if nav_button(ui, icon, selected, label).clicked() {
                                studio.tab = tab;
                            }
                        }
                        ui.add_space(28.0);
                        if nav_button(ui, 4, studio.show_history, "Chat history").clicked() {
                            studio.show_history = !studio.show_history;
                            studio.tab = Tab::Chat;
                        }
                        if nav_button(ui, 5, false, "Desktop mode").clicked() {
                            studio.mode = ViewMode::Desktop;
                        }
                    });
                });
            ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
                ui.add_space(18.0);
                ui.label(RichText::new("STUDIO").size(9.0).color(MUTED));
            });
        });

    if matches!(studio.tab, Tab::Chat) {
        egui::Area::new("brand".into())
            .fixed_pos(egui::pos2(left, 29.0))
            .show(ctx, |ui| {
                ui.label(
                    RichText::new("P R O J E C T")
                        .size(10.0)
                        .strong()
                        .color(MUTED),
                );
                ui.horizontal(|ui| {
                    ui.label(RichText::new("WAIFU").size(27.0).strong());
                    ui.label(RichText::new(".").size(29.0).strong().color(ACCENT));
                    if !narrow {
                        ui.add_space(18.0);
                        ui.label(RichText::new("/").size(23.0).color(LINE));
                        ui.add_space(18.0);
                        ui.label(
                            RichText::new(match studio.tab {
                                Tab::Chat => "Your conversation",
                                Tab::SceneChat => "Your scene conversation",
                                Tab::Characters => "Your profiles",
                                Tab::Scene => "Your space",
                                Tab::Connections => "Your connections",
                                Tab::Voice => "Your voice",
                                Tab::Memory => "Your memories",
                                Tab::Tools => "Your assistant",
                                Tab::Tasks => "Your agent tasks",
                                Tab::Extensions => "Your extensions",
                            })
                            .size(14.0)
                            .color(MUTED),
                        );
                    }
                });
            });
        egui::Area::new("connection_status".into())
            .anchor(egui::Align2::RIGHT_TOP, EVec2::new(-28.0, 45.0))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let color = if studio.connected {
                        Color32::from_rgb(145, 189, 149)
                    } else {
                        MUTED
                    };
                    let (rect, _) =
                        ui.allocate_exact_size(EVec2::splat(13.0), egui::Sense::hover());
                    ui.painter()
                        .circle_filled(rect.center(), 6.0, color.gamma_multiply(0.13));
                    ui.painter().circle_filled(rect.center(), 3.0, color);
                    ui.label(
                        RichText::new(if studio.connected {
                            "Connected"
                        } else {
                            "Not connected"
                        })
                        .size(13.0)
                        .color(MUTED),
                    );
                });
            });
    }

    if matches!(studio.tab, Tab::Chat) {
        egui::Area::new("character_heading".into())
            .fixed_pos(egui::pos2(left, 153.0))
            .show(ctx, |ui| {
                ui.set_max_width(if narrow { 210.0 } else { 330.0 });
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    ui.label(
                        RichText::new(&name)
                            .size(if narrow { 33.0 } else { 43.0 })
                            .strong(),
                    );
                    ui.label(RichText::new(".").size(43.0).strong().color(ACCENT));
                });
                ui.add_space(5.0);
                ui.label(RichText::new("Here with you").size(16.0).color(MUTED));
                ui.add_space(18.0);
                match &load.status {
                    LoadStatus::Loading => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(RichText::new("Loading your character").small().color(MUTED));
                        });
                    }
                    LoadStatus::Failed(error) => {
                        ui.colored_label(ACCENT, "Couldn't load this character")
                            .on_hover_text(error);
                    }
                    LoadStatus::Empty => {
                        ui.label(RichText::new("Bring a character into your space.").color(MUTED));
                        ui.add_space(10.0);
                        if ui.button("+  Add a character").clicked() {
                            studio.open_models();
                        }
                        if ui.small_button("Scan a model folder").clicked() {
                            studio.scan_folder();
                        }
                    }
                    LoadStatus::Ready => {}
                }
            });
        if studio.show_history {
            history(ctx, &studio, &name, left, screen.height());
        }
    } else {
        let settings_tab = studio.tab;
        let tab_key = format!("{settings_tab:?}");
        settings_page(&mut viewport_ui, |ui| {
            if matches!(settings_tab, Tab::Characters | Tab::Scene) {
                crate::avatar_preview::dock(ui, &mut avatar_ui.preview);
            }
            settings_kit::column(ui, |ui| settings_header(ui, &mut studio));
            let content = |ui: &mut egui::Ui| match studio.tab {
                Tab::Characters => {
                    let previous = studio.library.selected;
                    characters(ui, &mut studio);
                    if let Some(id) = previous
                        && !studio.library.characters.iter().any(|c| c.id == id)
                    {
                        semantic.forget(id);
                    }
                }
                Tab::Scene => {
                    scene(
                        ui,
                        &mut studio,
                        &mut animation,
                        &mut desktop,
                        &mut avatar_ui,
                    );
                }
                Tab::SceneChat => {
                    settings_pages::group_chat(
                        ui,
                        &mut studio,
                        &mut avatar_ui.scene_chat,
                        &mut avatar_ui.roster,
                    );
                    crate::scene_chat_controls::transcript(ui, &avatar_ui.scene_chat);
                    settings_kit::advanced(ui, "group_chat", |ui| {
                        crate::scene_chat_controls::ui(
                            ui,
                            &mut avatar_ui.scene_chat,
                            &studio.library,
                        );
                        ui.separator();
                        crate::scene_roster_controls::ui(ui, &mut avatar_ui.roster);
                    });
                }
                Tab::Connections => {
                    settings_pages::connections(ui, &mut studio, &listening);
                    settings_kit::advanced(ui, "connections", |ui| {
                        settings_pages::session_keys(ui, &mut studio, &mut listening);
                        connections(ui, &mut studio);
                        anchored_section(ui, "Connections", "Decision model connection", "");
                        let before = studio.library.decision.clone();
                        let config = &mut studio.library.decision;
                        field(ui, "Evaluation endpoint", &mut config.endpoint, "");
                        field(
                            ui,
                            "Key environment variable",
                            &mut config.key_env,
                            "AI_GATEWAY_API_KEY",
                        );

                        if before != studio.library.decision {
                            studio.save();
                        }
                    });
                }
                Tab::Voice => {
                    let ready = ready_call_outputs(
                        avatar_ui.speech_outputs.as_deref(),
                        avatar_ui.extension_runtime.as_deref(),
                        studio.library.selected,
                    );
                    let catalogs = &mut *avatar_ui.catalogs;
                    settings_pages::voice(
                        ui,
                        &mut studio,
                        &mut catalogs.tts,
                        &mut catalogs.stt,
                        &mut listening,
                        &mut avatar_ui.voice_library,
                        ready.as_ref().map(Vec::len),
                    );
                    if let Some(hotkeys) = avatar_ui.hotkeys.as_deref_mut() {
                        crate::voice_hotkeys::settings_ui(ui, hotkeys);
                    }
                    settings_kit::advanced(ui, "voice", |ui| {
                        anchored_section(
                            ui,
                            &tab_key,
                            "Speech",
                            "Give each character a voice, hosted or on your own machine.",
                        );
                        group(ui, "Speech tuning", |ui| {
                            voice_settings(ui, &mut studio, &mut avatar_ui.catalogs.tts)
                        });
                        group(ui, "Live stream status", |ui| {
                            crate::fish_live_controls::ui(ui, &mut fish);
                            crate::fish_ws_controls::ui(ui, &mut ws);
                        });
                        anchored_section(
                            ui,
                            &tab_key,
                            "Voice design",
                            "Paid Fish preview: generate candidate voices from a description.",
                        );
                        crate::voice_design_controls::ui(ui, &mut voice_design);
                        anchored_section(ui, &tab_key, "Microphone", "Microphone & transcription.");
                        crate::listen_controls::ui(
                            ui,
                            &mut listening,
                            studio.library.selected,
                            studio.composer.trim().is_empty(),
                            &mut avatar_ui.catalogs.stt,
                        );
                        if ui.button("Save microphone settings").clicked() {
                            studio.library.listen = listening.config.clone();
                            studio.save();
                        }
                    });
                }
                Tab::Extensions => {
                    let ready = ready_call_outputs(
                        avatar_ui.speech_outputs.as_deref(),
                        avatar_ui.extension_runtime.as_deref(),
                        studio.library.selected,
                    );
                    extension_page::ui(
                        ui,
                        &mut studio,
                        &mut avatar_ui.extensions,
                        &mut animation,
                        avatar_ui.extension_runtime.as_deref(),
                        &avatar_ui.facial.available,
                        ready.as_deref(),
                    );
                }
                Tab::Memory => memory_page::ui(ui, &mut studio, &mut memory, &mut semantic),
                Tab::Tools => {
                    settings_pages::tools(ui, &mut mcp, &mut studio);
                    if let Some(context) = mcp.take_context() {
                        if studio.composer.len() + context.len() <= 32_000 {
                            if !studio.composer.is_empty() {
                                studio.composer.push('\n');
                            }
                            studio.composer.push_str(&context);
                            studio.notice =
                                Some("Tool observation added. Review it before sending.".into());
                            studio.tab = Tab::Chat;
                        } else {
                            studio.error = Some("Tool result exceeds the 32 KB message limit. Copy a shorter excerpt.".into());
                        }
                    }
                }
                Tab::Tasks => {
                    settings_pages::tasks(ui, &mut studio);
                    ui.scope(|ui| {
                        group(ui, "Task loop", |ui| {
                            crate::orchestration::ui(
                                ui,
                                &mut orchestration,
                                &mut studio,
                                &mut agents,
                                &mut memory,
                            );
                        });
                        agent_tasks(ui, &mut studio, &mut agents, &mut board);
                        ui.separator();
                        group(ui, "Task board", |ui| {
                            crate::task_board_controls::ui(ui, &mut board, &agents)
                        });
                        let available = 32_000usize.saturating_sub(
                            studio.composer.len() + usize::from(!studio.composer.is_empty()),
                        );
                        if let Some(context) = board.try_take_context(available) {
                            append_context(&mut studio, &context);
                        } else if board.peek_context_len().is_some() {
                            kit::note(
                                ui,
                                "Result is ready. Shorten the draft to make room; the staged result is retained.",
                            );
                        }
                    });
                }
                Tab::Chat => {}
            };
            egui::ScrollArea::vertical()
                .id_salt(("settings_scroll", &tab_key))
                .auto_shrink([false, false])
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
                .show(ui, |ui| settings_kit::column(ui, content));
        });
    }

    if matches!(studio.tab, Tab::Chat) && (studio.error.is_some() || studio.notice.is_some()) {
        egui::Area::new("notice".into())
            .anchor(egui::Align2::RIGHT_TOP, EVec2::new(-25.0, 88.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                panel_frame().show(ui, |ui| {
                    ui.set_max_width(320.0);
                    if let Some(error) = studio.error.clone() {
                        ui.horizontal(|ui| {
                            ui.colored_label(ACCENT, "Something needs attention");
                            if ui.small_button("Dismiss").clicked() {
                                studio.error = None;
                            }
                        });
                        ui.label(error);
                    } else if let Some(notice) = studio.notice.clone() {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(notice);
                            if ui.small_button("Dismiss").clicked() {
                                studio.notice = None;
                            }
                        });
                    }
                });
            });
    }
    if matches!(studio.tab, Tab::Chat) {
        composer(
            ctx,
            &mut studio,
            &mut memory,
            &mut semantic,
            &mut listening,
            rail,
            screen.width(),
        );
    }
}

fn panel_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(Color32::from_rgb(29, 29, 29))
        .stroke(Stroke::new(1.0, LINE))
        .corner_radius(16)
        .inner_margin(20)
}
/// A real docked page. Floating Areas inherit an initial height constraint,
/// which previously left a half-height translucent settings window.
fn settings_page<R>(
    ui: &mut egui::Ui,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    egui::CentralPanel::default()
        .frame(egui::Frame::new().fill(SURFACE).inner_margin(20))
        .show(ui, |ui| {
            ui.spacing_mut().text_edit_width = control_width(ui);
            ui.spacing_mut().combo_width = control_width(ui);
            add(ui)
        })
}

fn settings_title(tab: Tab) -> (&'static str, &'static str) {
    match tab {
        Tab::Characters => (
            "Profiles",
            "Save a companion setup and switch whenever you like.",
        ),
        Tab::Scene => ("Scene & avatar", "How your companion looks and moves."),
        Tab::Connections => (
            "Connections",
            "Models and keys for everything the app talks to.",
        ),
        Tab::Voice => (
            "Voice & microphone",
            "How your companion sounds, and how you talk to them.",
        ),
        Tab::Memory => ("Memory", "What your companion remembers about you."),
        Tab::Extensions => ("Extensions", "Add content and manage installed packs."),
        Tab::Tools => ("Assistant tools", "Connect tools and review their results."),
        Tab::Tasks => (
            "Agent tasks",
            "Configure agents and review supervised work.",
        ),
        Tab::SceneChat => ("Group chat", "Talk with several companions at once."),
        Tab::Chat => ("Conversation", "Talk with your companion."),
    }
}

fn settings_header(ui: &mut egui::Ui, studio: &mut Studio) {
    let (title, description) = settings_title(studio.tab);
    egui::Sides::new().shrink_left().wrap().show(
        ui,
        |ui| {
            ui.label(RichText::new(title).size(23.0).strong());
        },
        |ui| {
            if ui.button("Back to chat").clicked() {
                studio.tab = Tab::Chat;
            }
        },
    );
    ui.label(RichText::new(description).color(MUTED));
    if studio.error.is_some() || studio.notice.is_some() {
        ui.add_space(6.0);
        egui::Frame::new()
            .fill(SURFACE)
            .inner_margin(10)
            .corner_radius(6)
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    let error = studio.error.is_some();
                    let message = studio
                        .error
                        .as_ref()
                        .or(studio.notice.as_ref())
                        .cloned()
                        .unwrap_or_default();
                    ui.label(RichText::new(message).color(if error { ACCENT } else { TEXT }));
                    if ui.small_button("Dismiss").clicked() {
                        if error {
                            studio.error = None;
                        } else {
                            studio.notice = None;
                        }
                    }
                });
            });
    }
    ui.add_space(8.0);
    ui.separator();
    ui.add_space(6.0);
}

fn jump_target(tab_key: &str) -> egui::Id {
    egui::Id::new(("settings_jump", tab_key))
}

/// Compact section heading; scrolls into view when its jump link was clicked.
fn anchored_section(ui: &mut egui::Ui, tab_key: &str, title: &str, description: &str) {
    ui.add_space(12.0);
    let heading = ui.label(RichText::new(title).size(17.0).strong());
    let target = jump_target(tab_key);
    if ui.ctx().data(|d| d.get_temp::<String>(target)).as_deref() == Some(title) {
        heading.scroll_to_me(Some(egui::Align::TOP));
        ui.ctx().data_mut(|d| d.remove::<String>(target));
    }
    if !description.is_empty() {
        ui.label(RichText::new(description).color(MUTED).size(13.0));
    }
    ui.separator();
    ui.add_space(6.0);
}

/// An open, framed group of related controls (collapsible, but visible by default).
fn group<R>(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui) -> R) {
    kit::group(ui, title, add);
}

/// WebWaifu-style caption above a control.
fn caption(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).size(12.0).strong().color(MUTED));
}

fn field(ui: &mut egui::Ui, label: &str, value: &mut String, hint: &str) {
    kit::text_row(ui, label, "", value, hint);
}

fn characters(ui: &mut egui::Ui, studio: &mut Studio) {
    crate::profile_library::gallery(ui, studio);
    character_editor(ui, studio);
}

/// Copies an original personality into `persona`. Empty text or an unmodified
/// original is replaced directly; custom text needs `confirmed`.
fn apply_original(
    persona: &mut String,
    original: &personality_catalog::OriginalPersonality,
    confirmed: bool,
) -> bool {
    let replaceable =
        persona.trim().is_empty() || personality_catalog::by_prompt(persona).is_some() || confirmed;
    if replaceable {
        original.system_prompt.clone_into(persona);
    }
    replaceable
}

fn character_editor(ui: &mut egui::Ui, studio: &mut Studio) {
    let model_choices: Vec<_> = studio
        .library
        .characters
        .iter()
        .map(|c| (c.model.clone(), c.format.clone()))
        .collect();
    let mut model_changed = false;
    let mut save = false;
    let mut notice = None;
    let mut action: Option<fn(&mut Studio)> = None;
    if let Some(character) = studio.library.selected_character_mut() {
        let suggested = personality_catalog::matching(&character.name, &character.model);
        // Automatic: a matching character with no personality gets its original
        // once per session. Text you typed is never replaced.
        let auto = egui::Id::new(("auto_personality", character.id));
        if let Some(original) = suggested
            && character.persona.trim().is_empty()
            && !ui.ctx().data(|d| d.get_temp::<bool>(auto).unwrap_or(false))
        {
            ui.ctx().data_mut(|d| d.insert_temp(auto, true));
            apply_original(&mut character.persona, original, false);
            save = true;
            notice = Some(format!(
                "Applied the original {} personality to {}.",
                original.name, character.name
            ));
        }

        settings_kit::section(ui, "Personality");
        let name = character.name.clone();
        settings_kit::card(ui, |c| {
            c.row("Name", "", |ui| {
                save |= ui
                    .add(
                        egui::TextEdit::singleline(&mut character.name)
                            .desired_width(ui.available_width().min(240.0)),
                    )
                    .lost_focus();
            });
            let current = personality_catalog::by_prompt(&character.persona);
            let selected_text = match current {
                Some(original) => original.name.to_owned(),
                None if character.persona.trim().is_empty() => "None".to_owned(),
                None => "Your own".to_owned(),
            };
            let hint = current.or(suggested).map_or(
                "Pick a ready-made personality, or write your own below.",
                |o| o.description,
            );
            let pending_id = egui::Id::new(("pending_personality", character.id));
            let mut chosen = None;
            c.row("Starting personality", hint, |ui| {
                egui::ComboBox::from_id_salt(("original_personality", character.id))
                    .selected_text(selected_text)
                    .truncate()
                    .width(ui.available_width().min(200.0))
                    .show_ui(ui, |ui| {
                        popup_layout(ui, 340.0);
                        for original in &personality_catalog::ORIGINALS {
                            let mut label = RichText::new(original.name).strong();
                            if suggested.is_some_and(|s| s.id == original.id) {
                                label = RichText::new(format!(
                                    "{} · matches this model",
                                    original.name
                                ))
                                .strong();
                            }
                            let picked = ui
                                .selectable_label(
                                    current.is_some_and(|c| c.id == original.id),
                                    label,
                                )
                                .on_hover_text(original.description)
                                .clicked();
                            if picked {
                                chosen = Some(original);
                                ui.close();
                            }
                        }
                    });
            });
            if let Some(original) = chosen {
                if apply_original(&mut character.persona, original, false) {
                    save = true;
                } else {
                    c.ui.ctx()
                        .data_mut(|d| d.insert_temp(pending_id, original.id.to_owned()));
                }
            }
            if let Some(pending) = c.ui.ctx().data(|d| d.get_temp::<String>(pending_id))
                && let Some(original) = personality_catalog::ORIGINALS
                    .iter()
                    .find(|p| p.id == pending)
            {
                c.row(
                    &format!("Replace your text with {}?", original.name),
                    "Your own personality text would be overwritten.",
                    |ui| {
                        ui.horizontal_wrapped(|ui| {
                            if ui.button("Keep mine").clicked() {
                                ui.ctx().data_mut(|d| d.remove::<String>(pending_id));
                            }
                            if ui.button("Replace").clicked() {
                                apply_original(&mut character.persona, original, true);
                                save = true;
                                ui.ctx().data_mut(|d| d.remove::<String>(pending_id));
                            }
                        });
                    },
                );
            }
            c.block(|ui| {
                ui.label(RichText::new("How they talk and behave").size(14.0));
                save |= ui
                    .add(
                        egui::TextEdit::multiline(&mut character.persona)
                            .desired_width(f32::INFINITY)
                            .desired_rows(5)
                            .hint_text("How should this character speak and behave?"),
                    )
                    .lost_focus();
            });
            c.block(|ui| {
                ui.label(RichText::new("First greeting").size(14.0));
                save |= ui
                    .add(
                        egui::TextEdit::multiline(&mut character.greeting)
                            .desired_width(f32::INFINITY)
                            .desired_rows(2)
                            .hint_text("Optional: what they say when a new chat starts"),
                    )
                    .lost_focus();
            });
        });
        settings_kit::section(ui, "Profile");
        let model = character
            .model
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let confirm_id = egui::Id::new(("confirm_remove_profile", character.id));
        let mut confirm = ui
            .ctx()
            .data(|d| d.get_temp::<bool>(confirm_id))
            .unwrap_or(false);
        settings_kit::card(ui, |c| {
            c.row("Avatar model", "Choose any avatar in your library.", |ui| {
                egui::ComboBox::from_id_salt(("profile_avatar", character.id))
                    .selected_text(&model)
                    .truncate()
                    .width(ui.available_width().min(240.0))
                    .show_ui(ui, |ui| {
                        let mut seen = std::collections::HashSet::new();
                        for (path, format) in &model_choices {
                            if !seen.insert(path.clone()) {
                                continue;
                            }
                            let name = path.file_name().unwrap_or_default().to_string_lossy();
                            if ui
                                .selectable_label(character.model == *path, name)
                                .clicked()
                            {
                                character.model = path.clone();
                                character.format = format.clone();
                                model_changed = true;
                                save = true;
                                ui.close();
                            }
                        }
                    });
            });
            c.row(
                "Duplicate",
                "A new profile with the same model, look and voice.",
                |ui| {
                    if ui.button("Duplicate").clicked() {
                        action = Some(Studio::duplicate_character);
                    }
                },
            );
            c.row(
                &format!("Remove {name}"),
                "Takes the profile out of your library. The model file stays on disk.",
                |ui| {
                    ui.horizontal_wrapped(|ui| {
                        if confirm {
                            if ui.button("Cancel").clicked() {
                                confirm = false;
                            }
                            if ui
                                .add(
                                    egui::Button::new(
                                        RichText::new("Remove").color(Color32::WHITE),
                                    )
                                    .fill(Color32::from_rgb(184, 88, 96)),
                                )
                                .clicked()
                            {
                                confirm = false;
                                action = Some(Studio::remove_character);
                            }
                        } else if ui.button("Remove…").clicked() {
                            confirm = true;
                        }
                    });
                },
            );
        });
        ui.ctx().data_mut(|d| d.insert_temp(confirm_id, confirm));
    }
    if notice.is_some() {
        studio.notice = notice;
    }
    if save {
        studio.save();
    }
    if model_changed && let Some(id) = studio.library.selected {
        studio.select_character(id);
    }
    if let Some(action) = action {
        action(studio);
    }
}

/// Form controls share one readable column instead of stretching across the page.
const FORM_WIDTH: f32 = 280.0;
fn control_width(ui: &egui::Ui) -> f32 {
    ui.available_width().clamp(1.0, FORM_WIDTH)
}

/// Bound popup contents to the viewport and wrap descriptions without widening menus.
fn popup_layout(ui: &mut egui::Ui, maximum: f32) {
    let width = (ui.ctx().content_rect().width() - 32.0).clamp(1.0, maximum);
    ui.set_width(width);
    ui.set_max_width(width);
    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
    let widgets = ui.ctx().style_of(egui::Theme::Dark).visuals.widgets.clone();
    ui.visuals_mut().widgets = widgets;
}

const ILLUSTRATED_BACKGROUNDS: [(&str, &str); 3] = [
    ("Cozy room", "cozy-room.png"),
    ("Moonlit garden", "moonlit-garden.png"),
    ("Neon rooftop", "neon-rooftop.png"),
];

fn scene_subnav(ui: &mut egui::Ui, id: &str, labels: &[&str]) -> usize {
    let key = egui::Id::new(("settings-category", id));
    let mut selected = ui.ctx().data(|d| d.get_temp::<usize>(key).unwrap_or(0));
    ui.horizontal_wrapped(|ui| {
        for (index, label) in labels.iter().enumerate() {
            ui.selectable_value(&mut selected, index, *label);
        }
    });
    ui.ctx().data_mut(|d| d.insert_temp(key, selected));
    ui.add_space(12.0);
    selected
}

fn scene(
    ui: &mut egui::Ui,
    studio: &mut Studio,
    animation: &mut AnimationState,
    desktop: &mut crate::desktop_settings::DesktopSettings,
    avatar: &mut AvatarUi,
) {
    let before = serde_json::to_string(&studio.library.scene).ok();
    let category = scene_subnav(
        ui,
        "scene",
        &[
            "Appearance",
            "Camera",
            "Motion",
            "Face & gaze",
            "Behavior",
            "World",
            "Desktop",
        ],
    );
    match category {
        0 => {
            kit::section(ui, "Appearance & post-processing");
            kit::card(ui, |c| {
                c.row(
                    "Shading",
                    "One base material style. Outlines and effects combine with any style.",
                    |ui| {
                        let material = &studio.library.scene.visuals.material;
                        let current = match studio.library.scene.shader {
                            1 => 3,
                            2 => 4,
                            _ if !material.mtoon => 0,
                            _ if material.toony >= 0.99 => 1,
                            _ => 2,
                        };
                        let mut selected = current;
                        egui::ComboBox::from_id_salt("appearance-shading")
                            .selected_text(
                                [
                                    "MToon · model original",
                                    "Cel shading",
                                    "Soft anime",
                                    "PBR",
                                    "Unlit",
                                ][current],
                            )
                            .width(control_width(ui))
                            .show_ui(ui, |ui| {
                                for (i, label) in [
                                    "MToon · model original",
                                    "Cel shading",
                                    "Soft anime",
                                    "PBR",
                                    "Unlit",
                                ]
                                .iter()
                                .enumerate()
                                {
                                    ui.selectable_value(&mut selected, i, *label);
                                }
                            });
                        if selected != current {
                            studio.library.scene.shader = match selected {
                                3 => 1,
                                4 => 2,
                                _ => 0,
                            };
                            if selected < 3 {
                                use waifu_studio::anime_look::AnimeLook;
                                let look = [AnimeLook::Authored, AnimeLook::Cel, AnimeLook::Soft]
                                    [selected];
                                studio.library.scene.visuals.material =
                                    look.apply(&studio.library.scene.visuals.material);
                            }
                        }
                    },
                );
                c.row(
                    "Anime outlines",
                    "Independent of MToon, cel, PBR and unlit shading.",
                    |ui| kit::toggle(ui, &mut studio.library.scene.outlines),
                );
                if c.row("Key light", "Lighting for the companion.", |ui| {
                    ui.add(egui::Slider::new(&mut desktop.light_intensity, 0.2..=2.0))
                })
                .changed()
                {
                    desktop.mark_dirty();
                }
                c.row(
                    "Restore model appearance",
                    "Keep the background and camera; restore authored materials and visible parts.",
                    |ui| {
                        if ui.button("Restore").clicked() {
                            studio.library.scene.shader = 0;
                            studio.library.scene.outlines = true;
                            studio.library.scene.visuals = Default::default();
                            avatar.appearance.config = Default::default();
                            studio.library.scene.appearance = Default::default();
                        }
                    },
                );
            });
            let mut state = waifu_studio::visuals::VisualState {
                settings: studio.library.scene.visuals.clone(),
                suppressed: studio.mode == ViewMode::Desktop,
            };
            if crate::visual_controls::ui(ui, &mut state) {
                studio.library.scene.visuals = state.settings;
            }
            group(ui, "Avatar parts", |ui| {
                if crate::appearance_controls::ui(ui, &mut avatar.appearance) {
                    studio.library.scene.appearance = avatar.appearance.config.clone();
                    studio.save();
                }
            });
        }
        1 => {
            kit::section(ui, "Camera & framing");
            kit::card(ui, |c| {
                c.row("Framing", "Choose the main Studio camera view.", |ui| {
                    for (value, label) in [(true, "Portrait"), (false, "Full body")] {
                        if ui
                            .selectable_value(&mut studio.library.scene.portrait, value, label)
                            .changed()
                        {
                            studio.frame_requested = true;
                        }
                    }
                });
                c.row("Recenter", "Return to the current framing.", |ui| {
                    if ui.button("Recenter").clicked() {
                        studio.frame_requested = true;
                    }
                });
                c.row("Settings preview", "Drag the preview to orbit; scroll to zoom. Its view is separate from the main Studio camera.", |ui| {ui.label("Drag / scroll preview");});
            });
            if kit::toggle_row(
                ui,
                "Wide room view",
                "Frame the furnished room around the companion.",
                &mut studio.library.scene.room_view,
            )
            .changed()
            {
                studio.frame_requested = true;
                studio.save();
            }
        }
        2 => {
            kit::section(ui, "Automatic motion & gestures");
            kit::card(ui, |c| {
                c.row(
                    "Breathing & sway",
                    "Subtle movement layered with the motion library.",
                    |ui| kit::toggle(ui, &mut studio.library.scene.idle_motion),
                );
            });
            animation_controls(ui, studio, animation);
            let mut physics = studio.library.scene.spring_physics.clone();
            if crate::spring_physics_controls::ui(ui, &mut physics, &avatar.physics) {
                studio.library.scene.spring_physics = physics;
            }
        }
        3 => {
            let facial = &mut avatar.facial;
            {
                if crate::expression_controls::ui(ui, facial) {
                    studio.library.scene.facial = facial.config.clone();
                    studio.save();
                }
            }
            {
                if crate::gaze_controls::ui(ui, &mut avatar.gaze) {
                    studio.library.scene.gaze = avatar.gaze.config.clone();
                    studio.save();
                }
            }
            {
                if crate::lipsync_controls::ui(ui, &mut avatar.visemes) {
                    studio.library.scene.visemes = avatar.visemes.config.clone();
                    studio.save();
                }
            }
        }
        4 => {
            let facial = &mut avatar.facial;
            {
                let selected = studio.library.selected;
                if crate::behavior_controls::ui(
                    ui,
                    &mut avatar.behavior,
                    &mut studio.library.scene.behavior,
                    selected,
                    &facial.available,
                ) {
                    studio.save();
                }
            }
            group(ui, "Fictional routine", |ui| {
                if crate::routine_controls::ui(ui, &mut studio.library.scene.routine) {
                    studio.save();
                }
            });
            group(ui, "Reply emotion & mood", |ui| {
                if let Some(id) = studio.library.selected {
                    let config = studio.library.scene.affect.clone();
                    let (previous, emotion, changed_config) = {
                        let state = studio.affect.entries.entry(id).or_default();
                        state.config = config;
                        let previous = state.emotion;
                        let changed =
                            waifu_studio::reply_affect_controls::draw_affect_controls(ui, state);
                        (
                            previous,
                            state.emotion,
                            changed.then(|| state.config.clone()),
                        )
                    };
                    if let Some(config) = changed_config {
                        studio.library.scene.affect = config;
                        if previous != emotion {
                            studio.lifecycle.record(
                                waifu_studio::extension_lifecycle::LifecycleEvent::EmotionChanged {
                                    character: id,
                                    owner: waifu_studio::extension_protocol::EmotionOwner::Studio,
                                    previous,
                                    emotion,
                                    reply_id: None,
                                },
                            );
                        }
                        studio.cancel_appraisal();
                        studio.save();
                    }
                    if studio.library.scene.affect.enabled && !studio.library.scene.behavior.enabled
                    {
                        ui.small("Enable Local behavior to show reply emotions on the avatar.");
                    }
                }
            });
            if !studio.appraisal_status.is_empty() {
                ui.small(&studio.appraisal_status);
            }

            kit::note(
                ui,
                format!(
                    "Reaction and motion decisions: {}",
                    studio.library.decision.model
                ),
            );
            if ui.button("Manage decision connection…").clicked() {
                studio.tab = Tab::Connections;
            }
        }
        5 => {
            let mut environment = waifu_studio::environments::EnvironmentSettings {
                kind: studio.library.scene.environment,
                suppressed: false,
            };
            if crate::environment_controls::ui(ui, &mut environment) {
                studio.library.scene.environment = environment.kind;
                studio.library.scene.room_view = matches!(
                    environment.kind,
                    waifu_studio::environments::EnvironmentKind::RoseBedroom
                        | waifu_studio::environments::EnvironmentKind::VioletStudio
                        | waifu_studio::environments::EnvironmentKind::CyanLounge
                );
                if studio.library.scene.room_view && !studio.library.scene.visuals.enabled {
                    use waifu_studio::visuals::{VisualPreset, VisualSettings};
                    studio.library.scene.visuals = VisualSettings::preset(
                        if environment.kind
                            == waifu_studio::environments::EnvironmentKind::RoseBedroom
                        {
                            VisualPreset::SoftAnime
                        } else {
                            VisualPreset::Neon
                        },
                    );
                }
                studio.frame_requested = true;
                studio.save();
            }
            let backgrounds = waifu_studio::bundled::assets_path().join("backgrounds");
            let current = studio.library.scene.image.clone();
            let current_label = match &current {
                None => "None (studio color)".to_owned(),
                Some(path) => ILLUSTRATED_BACKGROUNDS
                    .iter()
                    .find(|(_, file)| *path == backgrounds.join(file))
                    .map(|(label, _)| (*label).to_owned())
                    .unwrap_or_else(|| {
                        format!(
                            "Custom: {}",
                            path.file_name().unwrap_or_default().to_string_lossy()
                        )
                    }),
            };
            kit::row(ui, "Background image", "", |ui| {
                egui::ComboBox::from_id_salt("background_image")
                    .selected_text(current_label)
                    .truncate()
                    .width(control_width(ui))
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(current.is_none(), "None (studio color)")
                            .clicked()
                        {
                            studio.library.scene.image = None;
                            studio.save();
                        }
                        for (label, file) in ILLUSTRATED_BACKGROUNDS {
                            let path = backgrounds.join(file);
                            if ui
                                .selectable_label(current.as_ref() == Some(&path), label)
                                .clicked()
                            {
                                studio.library.scene.environment =
                                    waifu_studio::environments::EnvironmentKind::Disabled;
                                studio.library.scene.image = Some(path);
                                studio.save();
                            }
                        }
                        if ui
                            .selectable_label(false, "Choose your own image…")
                            .clicked()
                        {
                            studio.open_background();
                        }
                    });
            });
            kit::row(
                ui,
                "Studio color",
                "Shown when no image or room is active.",
                |ui| {
                    ui.color_edit_button_rgb(&mut studio.library.scene.background);
                },
            );
        }
        _ => {
            kit::section(ui, "Desktop companion");
            kit::card(ui, |c| {
                c.row(
                    "Desktop mode",
                    "Transparent companion with drag, taskbar seating and a radial menu.",
                    |ui| {
                        if ui.button("Open desktop").clicked() {
                            studio.mode = ViewMode::Desktop;
                        }
                    },
                );
            });
            desktop.ui(ui);
        }
    }
    if before != serde_json::to_string(&studio.library.scene).ok() {
        studio.save();
    }
}

/// Cached scan of the local motion folders; rescanned on demand.
fn motion_entries(ctx: &egui::Context, rescan: bool) -> motion_services::MotionEntries {
    motion_service_handles(ctx).entries(rescan)
}

fn filter_motions<'a>(
    entries: &'a [motion_library::MotionEntry],
    group: Option<&str>,
    query: &str,
    category: Option<motion_library::MotionCategory>,
) -> Vec<&'a motion_library::MotionEntry> {
    let query = query.trim().to_lowercase();
    entries
        .iter()
        .filter(|entry| group.is_none_or(|group| entry.group == group))
        .filter(|entry| category.is_none_or(|category| entry.category() == category))
        .filter(|entry| query.is_empty() || entry.label.to_lowercase().contains(&query))
        .collect()
}

/// Searchable motion selector; choosing an entry loads it through the existing transport.
fn motion_picker(
    ui: &mut egui::Ui,
    entries: &[motion_library::MotionEntry],
    current: Option<&std::path::Path>,
) -> Option<std::path::PathBuf> {
    let group_id = egui::Id::new("motion_library_group");
    let query_id = egui::Id::new("motion_library_query");
    let category_id = egui::Id::new("motion_library_category");
    let mut category = ui.ctx().data(|d| {
        d.get_temp::<Option<motion_library::MotionCategory>>(category_id)
            .flatten()
    });
    let mut group = ui
        .ctx()
        .data(|d| d.get_temp::<Option<String>>(group_id))
        .flatten();
    let mut query = ui
        .ctx()
        .data(|d| d.get_temp::<String>(query_id))
        .unwrap_or_default();
    let mut groups: Vec<&str> = entries.iter().map(|e| e.group.as_str()).collect();
    groups.sort_unstable();
    groups.dedup();
    let label = entries
        .iter()
        .find(|e| current == Some(e.path.as_path()))
        .map(|e| format!("{} · {}", e.label, e.format))
        .unwrap_or_else(|| "Choose a motion…".into());
    let mut picked = None;
    caption(ui, "Animation");
    let width = control_width(ui);
    egui::ComboBox::from_id_salt("motion_selector")
        .selected_text(label)
        .truncate()
        .height(440.0)
        .width(width)
        .show_ui(ui, |ui| {
            popup_layout(ui, 400.0);
            egui::ComboBox::from_id_salt("motion_folder")
                .selected_text(group.as_deref().unwrap_or("All folders"))
                .truncate()
                .width(ui.available_width())
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut group, None, "All folders");
                    for name in &groups {
                        ui.selectable_value(&mut group, Some((*name).to_owned()), *name);
                    }
                });
            egui::ComboBox::from_id_salt("motion_category")
                .selected_text(category.map_or("All categories", |c| c.name()))
                .truncate()
                .width(ui.available_width())
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut category, None, "All categories");
                    for value in motion_library::MotionCategory::ALL {
                        ui.selectable_value(&mut category, Some(value), value.name());
                    }
                });
            ui.small("Categories come from filenames; unknown motions remain available.");
            ui.add(
                egui::TextEdit::singleline(&mut query)
                    .desired_width(f32::INFINITY)
                    .hint_text("Search motions…"),
            );
            let visible = filter_motions(entries, group.as_deref(), &query, category);
            ui.small(format!("{} motions", visible.len()));
            egui::ScrollArea::vertical()
                .id_salt("motion_library_rows")
                .max_height(300.0)
                .show_rows(ui, 34.0, visible.len(), |ui, range| {
                    for entry in &visible[range] {
                        if ui
                            .add_sized(
                                [ui.available_width(), 34.0],
                                egui::Button::selectable(
                                    current == Some(entry.path.as_path()),
                                    format!("{} · {} · {}", entry.label, entry.format, entry.group),
                                )
                                .truncate(),
                            )
                            .on_hover_text(entry.path.display().to_string())
                            .clicked()
                        {
                            picked = Some(entry.path.clone());
                            ui.close();
                        }
                    }
                });
            if visible.is_empty() {
                ui.small("No matching motions.");
            }
        });
    ui.ctx().data_mut(|d| {
        d.insert_temp(group_id, group);
        d.insert_temp(query_id, query);
        d.insert_temp(category_id, category);
    });
    picked
}

enum SuggestAction {
    Start(motion_decision::SuggestRequest),
    Play(std::path::PathBuf),
    Error(String),
}

/// UI adapters hold clones of the Bevy resource's shared handles. Standalone
/// UI tests use a context-local default until a production resource is bound.
fn motion_service_handles(ctx: &egui::Context) -> MotionServices {
    ctx.data_mut(|data| {
        data.get_temp_mut_or_default::<MotionServices>(egui::Id::new("motion_services"))
            .clone()
    })
}

fn bind_motion_services(ctx: &egui::Context, services: &MotionServices) {
    ctx.data_mut(|data| {
        data.insert_temp(egui::Id::new("motion_services"), services.clone());
    });
}

fn motion_shared(ctx: &egui::Context) -> (SharedSuggester, SharedReplyMotion) {
    let services = motion_service_handles(ctx);
    (services.suggester, services.reply)
}

#[cfg(test)]
fn seed_motion_test_catalog(ctx: &egui::Context, services: &MotionServices) {
    if let Some(entries) = ctx.data(|data| {
        data.get_temp::<motion_services::MotionEntries>(egui::Id::new("motion_library_entries"))
    }) {
        services.replace_entries(entries);
    }
}

fn lock<T>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Any manual motion action cancels or discards an automatic pick.
fn manual_motion(ctx: &egui::Context, animation: &mut AnimationState) {
    motion_services::manual_motion(&motion_service_handles(ctx), animation);
}

fn companion_shared(ctx: &egui::Context) -> SharedCompanion {
    motion_service_handles(ctx).companion
}

#[cfg(test)]
fn companion_motion_frame(
    ctx: &egui::Context,
    studio: &Studio,
    animation: &mut AnimationState,
    avatar: Option<u64>,
    playing: bool,
) {
    let services = motion_service_handles(ctx);
    seed_motion_test_catalog(ctx, &services);
    motion_services::companion_motion_frame(&services, studio, animation, avatar, playing);
}

fn companion_motion_controls(
    ui: &mut egui::Ui,
    studio: &mut Studio,
    animation: &mut AnimationState,
    entries: &[motion_library::MotionEntry],
) {
    group(ui, "Idle & talking animator", |ui| {
        let config = &mut studio.library.scene.companion_motion;
        let mut relocated = false;
        for path in config
            .idle_paths
            .iter_mut()
            .chain(config.talking_paths.iter_mut())
        {
            if let Some(entry) = motion_library::saved_motion(entries, path)
                && entry.path != *path
            {
                *path = entry.path.clone();
                relocated = true;
            }
        }
        let mut changed = kit::toggle_row(
            ui,
            "Automatic idle and talking motions",
            "Emotion gestures finish first; speech then resumes talking, or idle when quiet.",
            &mut config.enabled,
        )
        .changed()
            || relocated;
        for (title, talking) in [("Idle playlist", false), ("Talking playlist", true)] {
            let paths = if talking {
                &mut config.talking_paths
            } else {
                &mut config.idle_paths
            };
            ui.collapsing(title, |ui| {
                kit::note(ui, "Empty uses matching filenames automatically. Checked motions override that list; seated/locomotion motions are excluded.");
                egui::ScrollArea::vertical().id_salt(title).max_height(180.0).show(ui, |ui| {
                    for entry in entries.iter().filter(|entry| entry.auto_playable() && !["sit", "kneel", "lay", "crawl"].iter().any(|word| entry.label.to_lowercase().contains(word))) {
                        let mut selected = paths.contains(&entry.path);
                        if ui.checkbox(&mut selected, &entry.label).changed() {
                            paths.retain(|path| path != &entry.path);
                            if selected { paths.push(entry.path.clone()); }
                            changed = true;
                        }
                    }
                });
                if ui.button("Use automatic category list").clicked() { paths.clear(); changed = true; }
            });
        }
        let shared = companion_shared(ui.ctx());
        let mut controller = lock(&shared);
        if controller.paused && ui.button("Resume automatic animator").clicked() {
            animation.resume_automatic_control();
            controller.resume();
        }
        if let Some(note) = &controller.note {
            kit::note(ui, note);
        }
        if changed {
            animation.resume_automatic_control();
            controller.resume();
            studio.save();
        }
    });
}

/// Root-level, non-blocking reply-motion step. Off does nothing but remember
/// which reply was last seen; it never scans, requests or plays.
#[cfg(test)]
fn reply_motion_frame(
    ctx: &egui::Context,
    studio: &mut Studio,
    animation: &mut AnimationState,
    avatar: Option<u64>,
    desktop_pose_owned: bool,
) {
    let services = motion_service_handles(ctx);
    seed_motion_test_catalog(ctx, &services);
    motion_services::reply_motion_frame(&services, studio, animation, avatar, desktop_pose_owned);
}

/// Response-gesture mode plus manual suggestions. Merely viewing this panel
/// never sends a request; reviewed manual suggestions require Play.
#[allow(clippy::too_many_arguments)]
fn motion_suggestion(
    ui: &mut egui::Ui,
    suggester: &mut motion_decision::MotionSuggester,
    auto: &mut motion_decision::ReplyMotion,
    config: &mut waifu_studio::data::ReplyMotionConfig,
    entries: &[motion_library::MotionEntry],
    character: Option<(uuid::Uuid, &str)>,
    draft: &str,
    reply: Option<&str>,
    model_picker: impl FnOnce(&mut egui::Ui),
) -> (Option<SuggestAction>, bool) {
    use waifu_studio::data::ReplyMotionMode;
    let source_id = egui::Id::new("motion_suggest_use_reply");
    let mut use_reply = ui
        .ctx()
        .data(|d| d.get_temp::<bool>(source_id))
        .unwrap_or(false);
    let mut action = None;
    let mut changed = false;
    kit::group(ui, "Emotion-driven gestures", |ui| {
        kit::note(
            ui,
            "Your decision model chooses from the scanned stationary motions, or none. Manual suggestions require Play.",
        );
        model_picker(ui);
        let mode_name = |mode| match mode {
            ReplyMotionMode::Off => "Off",
            ReplyMotionMode::Suggest => "Suggest after reply",
            ReplyMotionMode::AutoPlay => "Decision model · automatic",
            ReplyMotionMode::EmotionMapped => "Play bound motion for reply emotion",
        };
        kit::row(ui, "Emotion gestures", "", |ui| {
            egui::ComboBox::from_id_salt("reply_motion_mode")
                .width(control_width(ui))
                .truncate()
                .selected_text(mode_name(config.mode))
                .show_ui(ui, |ui| {
                    for mode in [
                        ReplyMotionMode::Off,
                        ReplyMotionMode::Suggest,
                        ReplyMotionMode::AutoPlay,
                        ReplyMotionMode::EmotionMapped,
                    ] {
                        changed |= ui
                            .selectable_value(&mut config.mode, mode, mode_name(mode))
                            .changed();
                    }
                });
        });
        if config.mode != ReplyMotionMode::Off {
            let mut seconds = config.cooldown_seconds.clamp(
                motion_decision::MIN_COOLDOWN_SECONDS,
                motion_decision::MAX_COOLDOWN_SECONDS,
            );
            if kit::slider_row(
                ui,
                "Cooldown",
                "Minimum time between automatic motion requests.",
                egui::Slider::new(
                    &mut seconds,
                    motion_decision::MIN_COOLDOWN_SECONDS..=motion_decision::MAX_COOLDOWN_SECONDS,
                )
                .suffix(" s"),
            )
            .changed()
            {
                config.cooldown_seconds = seconds;
                changed = true;
            }
            if config.mode == ReplyMotionMode::EmotionMapped {
                kit::note(
                    ui,
                    "Uses the incoming emotion appraisal or reply affect to rotate through your selected motions. No additional motion-selection request is sent. Emotions without a binding play nothing; walking remains manual.",
                );
                changed |= emotion_bindings(ui, config, entries);
            } else {
                kit::note(
                    ui,
                    "Chooses a gesture from the appraised emotion and current message while the reply starts, or from the completed reply when no appraisal is available. The gesture finishes, then talking or idle motion resumes. Does not delay speech. Walking remains manual.",
                );
            }
            if let Some(note) = &auto.note {
                ui.small(RichText::new(note).color(MUTED));
            }
        }
        kit::row(ui, "Suggestion context", "", |ui| {
            egui::ComboBox::from_id_salt("motion_suggest_source")
                .width(control_width(ui))
                .truncate()
                .selected_text(if use_reply {
                    "Latest reply"
                } else {
                    "Composer draft"
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut use_reply, false, "Composer draft");
                    ui.selectable_value(&mut use_reply, true, "Latest reply");
                });
        });
        let context = if use_reply {
            reply.unwrap_or("")
        } else {
            draft
        };
        let context = motion_decision::bounded_context(context);
        match suggester.status().clone() {
            motion_decision::SuggestStatus::Running => {
                ui.horizontal_wrapped(|ui| {
                    ui.spinner();
                    ui.label("Choosing a motion…");
                    if ui.button("Cancel").clicked() {
                        auto.manual_override(suggester);
                        suggester.cancel();
                    }
                });
            }
            motion_decision::SuggestStatus::Cancelling => {
                ui.horizontal_wrapped(|ui| {
                    ui.spinner();
                    ui.label("Cancelling; the result will be ignored.");
                });
            }
            status => {
                let ready = character.is_some() && !context.is_empty();
                let button = ui
                    .add_enabled(ready, egui::Button::new("Suggest motion"))
                    .on_disabled_hover_text(
                        "Select a character and write a draft or get a reply first.",
                    );
                if button.clicked()
                    && let Some((id, name)) = character
                {
                    action = Some(SuggestAction::Start(motion_decision::SuggestRequest {
                        character: id,
                        character_name: name.to_owned(),
                        context: context.to_owned(),
                        catalog: motion_library::MotionCatalog::offer(
                            entries,
                            &SUGGESTED_CATEGORIES,
                            motion_library::MAX_OFFERED,
                        ),
                    }));
                }
                match status {
                    motion_decision::SuggestStatus::Ready(suggestion) => {
                        if let Some(label) = &suggestion.label {
                            ui.label(format!("Suggested: {label}"));
                            ui.horizontal_wrapped(|ui| {
                                if ui.button("Play suggestion").clicked() {
                                    action = Some(
                                        match suggester
                                            .resolve_play(character.map(|(id, _)| id), entries)
                                        {
                                            Ok(path) => SuggestAction::Play(path),
                                            Err(error) => SuggestAction::Error(error),
                                        },
                                    );
                                }
                                if ui.button("Dismiss").clicked() {
                                    suggester.dismiss();
                                }
                            });
                        } else {
                            ui.label(RichText::new("No offered motion fits.").color(MUTED));
                        }
                    }
                    motion_decision::SuggestStatus::Failed(error) => {
                        ui.colored_label(ACCENT, error);
                    }
                    _ => {}
                }
            }
        }
    });
    ui.ctx().data_mut(|d| d.insert_temp(source_id, use_reply));
    (action, changed)
}

/// Multi-select dropdown per emotion. Stale bindings remain visible and can be
/// removed, but only scanned non-locomotion entries are offered for new bindings.
fn emotion_bindings(
    ui: &mut egui::Ui,
    config: &mut waifu_studio::data::ReplyMotionConfig,
    entries: &[motion_library::MotionEntry],
) -> bool {
    let mut changed = false;
    for binding in &mut config.emotion_bindings {
        if let Some(entry) = motion_library::saved_motion(entries, &binding.path)
            && entry.path != binding.path
        {
            binding.path = entry.path.clone();
            changed = true;
        }
    }
    caption(ui, "Emotion playlists");
    kit::note(
        ui,
        "Check one or more motions per emotion. Eligible replies rotate in selection order; the cooldown still applies.",
    );
    for emotion in waifu_studio::reply_affect::AffectEmotion::ALL {
        let mut current = Vec::new();
        for binding in config
            .emotion_bindings
            .iter()
            .filter(|b| b.emotion == emotion)
        {
            if !current.contains(&binding.path) {
                current.push(binding.path.clone());
            }
        }
        let eligible = |path: &std::path::PathBuf| {
            entries
                .iter()
                .find(|entry| &entry.path == path && entry.auto_playable())
        };
        let missing: Vec<_> = current
            .iter()
            .filter(|path| eligible(path).is_none())
            .cloned()
            .collect();
        let text = match current.as_slice() {
            [] => "None".to_owned(),
            [path] => eligible(path).map_or_else(
                || "Missing from scan".to_owned(),
                |entry| entry.label.clone(),
            ),
            _ if missing.is_empty() => format!("{} motions", current.len()),
            _ => format!("{} motions ({} unavailable)", current.len(), missing.len()),
        };
        let mut picked = current.clone();
        kit::row(ui, emotion.as_str(), "", |ui| {
            egui::ComboBox::from_id_salt(("emotion_binding", emotion.as_str()))
                .selected_text(text)
                .width(control_width(ui))
                .truncate()
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                .show_ui(ui, |ui| {
                    popup_layout(ui, 380.0);
                    if ui.selectable_label(picked.is_empty(), "None").clicked() {
                        picked.clear();
                    }
                    for path in &missing {
                        let mut keep = picked.contains(path);
                        let label = format!(
                            "Unavailable: {}",
                            path.file_name().unwrap_or_default().to_string_lossy()
                        );
                        if ui.checkbox(&mut keep, label).changed() {
                            if keep {
                                picked.push(path.clone());
                            } else {
                                picked.retain(|p| p != path);
                            }
                        }
                    }
                    for entry in entries.iter().filter(|entry| entry.auto_playable()) {
                        let mut checked = picked.contains(&entry.path);
                        if ui
                            .checkbox(&mut checked, format!("{} / {}", entry.group, entry.label))
                            .changed()
                        {
                            if checked {
                                picked.push(entry.path.clone());
                            } else {
                                picked.retain(|p| p != &entry.path);
                            }
                        }
                    }
                });
        });
        if picked != current {
            changed = true;
            config
                .emotion_bindings
                .retain(|binding| binding.emotion != emotion);
            config.emotion_bindings.extend(
                picked
                    .into_iter()
                    .map(|path| waifu_studio::data::EmotionMotionBinding { emotion, path }),
            );
        }
    }
    changed
}

fn suggest_motion(
    ui: &mut egui::Ui,
    studio: &mut Studio,
    animation: &mut AnimationState,
    entries: &[motion_library::MotionEntry],
) {
    let (shared, auto) = motion_shared(ui.ctx());
    let mut suggester = lock(&shared);
    let mut auto = lock(&auto);
    let selected = studio.library.selected;
    suggester.poll(selected, entries);
    let character = selected
        .and_then(|id| studio.library.characters.iter().find(|c| c.id == id))
        .map(|c| (c.id, c.name.as_str()));
    let reply = studio
        .turns
        .iter()
        .rev()
        .find(|turn| turn.role == "assistant")
        .map(|turn| turn.content.as_str());
    let mut config = studio.library.scene.reply_motion.clone();
    let connection = studio.library.decision.clone();
    let (action, changed) = motion_suggestion(
        ui,
        &mut suggester,
        &mut auto,
        &mut config,
        entries,
        character,
        &studio.composer,
        reply,
        |ui| {
            kit::note(
                ui,
                format!(
                    "Decision model: {} · managed in Connections",
                    connection.model
                ),
            );
        },
    );
    if changed {
        studio.library.scene.reply_motion = config;
        studio.save();
    }
    if action.is_some() {
        auto.manual_override(&mut suggester);
    }
    match action {
        Some(SuggestAction::Start(request)) => {
            if let Err(error) = suggester.start(&studio.library.decision, request) {
                studio.error = Some(error);
            }
        }
        Some(SuggestAction::Play(path)) => {
            if let Err(error) = animation.load_one_shot(path) {
                studio.error = Some(error);
            }
        }
        Some(SuggestAction::Error(error)) => studio.error = Some(error),
        None => {}
    }
    if suggester.busy() {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(100));
    }
}

fn animation_controls(ui: &mut egui::Ui, studio: &mut Studio, animation: &mut AnimationState) {
    let current_entries = motion_entries(ui.ctx(), false);
    if !current_entries.is_empty() {
        companion_motion_controls(ui, studio, animation, &current_entries);
        suggest_motion(ui, studio, animation, &current_entries);
    }
    kit::section(ui, "Manual motion library");
    let rescan = ui
        .horizontal_wrapped(|ui| {
            if ui
                .add_enabled(!animation.loading, egui::Button::new("Import motion file…"))
                .clicked()
            {
                manual_motion(ui.ctx(), animation);
                studio.open_animation();
            }
            ui.button("Rescan motion library").clicked()
        })
        .inner;
    let entries = motion_entries(ui.ctx(), rescan);
    if entries.is_empty() {
        ui.label(
            RichText::new(
                "No local motion library found. Add folders with WAIFU_MOTION_LIBRARY, or import a file.",
            )
            .color(MUTED),
        );
    } else if let Some(path) = motion_picker(ui, &entries, animation.path.as_deref()) {
        manual_motion(ui.ctx(), animation);
        if let Err(error) = animation.load_path(path) {
            studio.error = Some(error);
        }
    }
    if animation.loading {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label("Loading motion...");
        });
    }
    if let Some(error) = &animation.error {
        ui.colored_label(ACCENT, error);
    }
    caption(ui, "Playback");
    if let Some(clip) = &animation.clip {
        let duration = clip.duration();
        ui.label(
            RichText::new(format!(
                "{} · {} · {} frames · {:.1} seconds",
                animation
                    .path
                    .as_ref()
                    .and_then(|p| p.file_name())
                    .unwrap_or_default()
                    .to_string_lossy(),
                clip.format(),
                clip.frame_count,
                duration
            ))
            .color(MUTED),
        );
        ui.horizontal_wrapped(|ui| {
            if animation.playing {
                if ui.button("Pause").clicked() {
                    manual_motion(ui.ctx(), animation);
                    animation.pause();
                }
            } else if ui.button("Play").clicked() {
                manual_motion(ui.ctx(), animation);
                animation.play();
            }
            if ui.button("Stop").clicked() {
                manual_motion(ui.ctx(), animation);
                animation.stop();
            }
        });
        if kit::toggle_row(
            ui,
            "Loop",
            "Repeat manually selected playback.",
            &mut animation.looping,
        )
        .changed()
        {
            manual_motion(ui.ctx(), animation);
        }
        ui.spacing_mut().slider_width = (control_width(ui) - 120.0).max(160.0);
        if kit::slider_row(
            ui,
            "Position",
            "",
            egui::Slider::new(&mut animation.position, 0.0..=duration).suffix(" s"),
        )
        .changed()
        {
            manual_motion(ui.ctx(), animation);
            if !animation.is_posing() {
                animation.show_scrub_pose();
            }
        }
        if kit::slider_row(
            ui,
            "Speed",
            "",
            egui::Slider::new(&mut animation.speed, 0.1..=2.0).suffix("×"),
        )
        .changed()
        {
            manual_motion(ui.ctx(), animation);
        }
        if kit::slider_row(
            ui,
            "Motion weight",
            "",
            egui::Slider::new(&mut animation.weight, 0.0..=1.0),
        )
        .changed()
        {
            manual_motion(ui.ctx(), animation);
        }
        kit::toggle_row(
            ui,
            "Arm clearance (experimental)",
            "Try whole-arm clearance against the torso mesh. Can distort gestures; does not protect the full movement path. Resets when the app closes.",
            &mut animation.arm_clearance,
        );
        kit::toggle_row(
            ui,
            "Soft arm clearance (experimental)",
            "Swing the whole arm slightly away when it would pass through the body. Keeps the gesture's shape; ignores skirts and hair. Resets when the app closes.",
            &mut animation.soft_arm_clearance,
        );
        if animation.is_posing() && !animation.playing {
            ui.label(RichText::new("Paused on this pose").small().color(MUTED));
        }
    } else {
        ui.label(
            RichText::new("No motion loaded. Choose an animation from the dropdown.").color(MUTED),
        );
    }
    ui.label(
        RichText::new("Rest-normalized rotation and scaled hips translation. FBX uses the first stack and one baked layer. Library files are read in place, never copied.")
            .small()
            .color(MUTED),
    );
}

fn connections(ui: &mut egui::Ui, studio: &mut Studio) {
    const KEY: &str = "Connections";
    anchored_section(
        ui,
        KEY,
        "Models",
        "Refresh the catalog for the selected provider.",
    );
    group(ui, "Model selection", |ui| {
        studio.ensure_models();
        model_discovery_status(ui, studio);
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    !studio.models_status().0,
                    egui::Button::new("Refresh models"),
                )
                .clicked()
            {
                studio.refresh_models();
            }
        });
    });
    anchored_section(
        ui,
        KEY,
        "API & routing",
        "Endpoint, credentials and upstream routing.",
    );
    group(ui, "Connection details", |ui| {
        crate::provider_controls::ui(ui, studio);
        let index = studio.library.provider;
        if let Some(provider) = studio.library.providers.get_mut(index) {
            field(
                ui,
                "Endpoint",
                &mut provider.base_url,
                "http://127.0.0.1:11434/v1",
            );

            field(
                ui,
                "Key environment variable",
                &mut provider.key_env,
                "AI_GATEWAY_API_KEY",
            );
        }
        if ui.button("Save connection").clicked() {
            studio.save();
        }
    });
}

fn voice_settings(
    ui: &mut egui::Ui,
    studio: &mut Studio,
    catalog: &mut waifu_studio::model_catalog::ModelCatalog,
) {
    if ui.button("Use live speech pipeline").on_hover_text("Fish streams text and audio while the LLM responds. Keeps the selected model, voice, speed and automatic-speech toggle. Other adapters use their supported delivery.").clicked() {
        studio.stop_voice();
        studio.library.voice.use_live_pipeline();
        studio.save();
    }
    if studio.library.voice.adapter == VoiceAdapter::FishHosted
        && ui.button("Try HTTP low latency")
            .on_hover_text("Applies the HTTP/Low tuning from recent latency tests. Keeps your model, voice, speed and automatic-speech choice. Does not generate speech.")
            .clicked()
    {
        studio.stop_voice();
        studio.library.voice.use_http_low_latency();
        studio.save();
    }
    let voice = &mut studio.library.voice;
    if voice.adapter != VoiceAdapter::FishLocal {
        catalog.sync(
            &voice.endpoint,
            &voice.api_key_env,
            &studio.voice_key,
            waifu_studio::model_catalog::CatalogKind::Speech,
        );
        if ui.button("Refresh speech models").clicked() {
            catalog.retry(
                &voice.endpoint,
                &voice.api_key_env,
                &studio.voice_key,
                waifu_studio::model_catalog::CatalogKind::Speech,
            );
        }
    }

    field(
        ui,
        "Speech endpoint",
        &mut voice.endpoint,
        "http://127.0.0.1:8080/v1/tts",
    );
    match voice.adapter {
        VoiceAdapter::FishHosted => {
            kit::note(
                ui,
                "Free tier: s2.1-pro-free. Other models use your Fish account pricing.",
            );
            if voice.model == "drama-3" {
                field(
                    ui,
                    "Performance direction",
                    &mut voice.direction,
                    "Warm, relaxed, conversational",
                );
            } else {
                voice.direction.clear();
            }
        }
        VoiceAdapter::FishLocal => {
            ui.label(
                RichText::new("Uses the speech model loaded in your Fish Speech server.")
                    .small()
                    .color(MUTED),
            );
        }
        VoiceAdapter::OpenAi | VoiceAdapter::Vercel => {
            if voice.adapter == VoiceAdapter::Vercel {
                field(
                    ui,
                    "Voice direction",
                    &mut voice.direction,
                    "Optional instructions, where supported by the selected model",
                );
                voice.delivery = waifu_studio::voice::VoiceDelivery::Buffered;
                kit::note(
                    ui,
                    "Vercel speech uses buffered audio. Voice IDs and formats depend on the selected model.",
                );
            }
        }
    }
    if matches!(
        voice.adapter,
        VoiceAdapter::FishHosted | VoiceAdapter::FishLocal
    ) {
        kit::row(ui, "Delivery", "", |ui| {
            egui::ComboBox::from_id_salt("Delivery")
                .width(control_width(ui))
                .truncate()
                .selected_text(match voice.delivery {
                    waifu_studio::voice::VoiceDelivery::Buffered => "Buffered",
                    waifu_studio::voice::VoiceDelivery::HttpWavStream => "Progressive HTTP WAV",
                    waifu_studio::voice::VoiceDelivery::TimestampSse => "Fish timestamp SSE",
                    waifu_studio::voice::VoiceDelivery::LiveWebSocket => {
                        "Live text/audio WebSocket"
                    }
                    waifu_studio::voice::VoiceDelivery::TimestampWebSocket => {
                        "Live WebSocket + timestamps"
                    }
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut voice.delivery,
                        waifu_studio::voice::VoiceDelivery::Buffered,
                        "Buffered",
                    );
                    ui.selectable_value(
                        &mut voice.delivery,
                        waifu_studio::voice::VoiceDelivery::HttpWavStream,
                        "Progressive HTTP WAV",
                    );
                    if voice.adapter == VoiceAdapter::FishHosted {
                        ui.selectable_value(
                            &mut voice.delivery,
                            waifu_studio::voice::VoiceDelivery::TimestampSse,
                            "Fish timestamp SSE",
                        );
                        ui.selectable_value(
                            &mut voice.delivery,
                            waifu_studio::voice::VoiceDelivery::LiveWebSocket,
                            "Live text/audio WebSocket",
                        );
                        ui.selectable_value(
                            &mut voice.delivery,
                            waifu_studio::voice::VoiceDelivery::TimestampWebSocket,
                            "Live WebSocket + timestamps",
                        );
                    }
                });
        });
        kit::row(ui, "Latency", "", |ui| {
            egui::ComboBox::from_id_salt("Latency")
                .width(control_width(ui))
                .truncate()
                .selected_text(&voice.latency)
                .show_ui(ui, |ui| {
                    for latency in ["normal", "balanced", "low"] {
                        if latency != "low" || voice.adapter == VoiceAdapter::FishHosted {
                            ui.selectable_value(&mut voice.latency, latency.to_owned(), latency);
                        }
                    }
                });
        });
        let maximum = if voice.adapter == VoiceAdapter::FishHosted {
            300
        } else {
            1000
        };
        kit::slider_row(
            ui,
            "Chunk length",
            "",
            egui::Slider::new(&mut voice.chunk_length, 100..=maximum),
        );
        if voice.adapter == VoiceAdapter::FishHosted {
            kit::slider_row(
                ui,
                "Minimum chunk length",
                "",
                egui::Slider::new(&mut voice.min_chunk_length, 0..=100),
            );
            kit::toggle_row(
                ui,
                "Condition on previous audio chunks",
                "",
                &mut voice.condition_on_previous_chunks,
            );
        }
    }
    if voice.adapter == VoiceAdapter::Vercel {
        voice.delivery = waifu_studio::voice::VoiceDelivery::Buffered;
    }
    if voice.delivery != waifu_studio::voice::VoiceDelivery::Buffered {
        voice.format = "wav".into();
    }
    kit::row(ui, "Audio format", "", |ui| {
        egui::ComboBox::from_id_salt("voice_format")
            .width(control_width(ui))
            .truncate()
            .selected_text(&voice.format)
            .show_ui(ui, |ui| {
                for format in ["wav", "mp3"] {
                    if ui
                        .selectable_label(voice.format == format, format)
                        .clicked()
                    {
                        voice.format = format.into();
                    }
                }
            });
    });
    if voice.delivery != waifu_studio::voice::VoiceDelivery::Buffered {
        voice.format = "wav".into();
    }

    if voice.adapter == VoiceAdapter::FishLocal {
        ui.small("Local speed changes playback rate and pitch.");
    }
    field(
        ui,
        "Key environment variable",
        &mut voice.api_key_env,
        "Optional for local speech",
    );

    ui.horizontal_wrapped(|ui| {
        if ui.button("Preview local WAV/MP3…").clicked() {
            studio.preview_audio_file();
        }
        if ui.button("Stop speech").clicked() {
            studio.stop_voice();
        }
        if ui.button("Save voice").clicked() {
            studio.save();
        }
    });
    if studio.voice_busy {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(RichText::new("Preparing speech...").small().color(MUTED));
        });
    }
}

fn conversation_turn_label(turn: &waifu_studio::data::Turn, name: &str) -> String {
    if turn.role == "external" || turn.external.is_some() {
        return match &turn.external {
            Some(metadata) if turn.validate_attribution().is_ok() => {
                format!("External · {}", metadata.origin.display_name)
            }
            _ => "External · Unattributed".into(),
        };
    }
    match turn.role.as_str() {
        "user" => "You".into(),
        "assistant" | "assistant_progress" => name.into(),
        _ => "Message".into(),
    }
}

fn history(ctx: &egui::Context, studio: &Studio, name: &str, left: f32, height: f32) {
    egui::Area::new("history_panel".into())
        .fixed_pos(egui::pos2(left, 285.0))
        .show(ctx, |ui| {
            panel_frame().show(ui, |ui| {
                ui.set_width((ctx.content_rect().right() - left - 48.0).clamp(1.0, 290.0));
                ui.label(RichText::new("Conversation").small().color(MUTED));
                egui::ScrollArea::vertical()
                    .id_salt("conversation_history")
                    .max_height((height - 490.0).max(90.0))
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        if studio.turns.is_empty() {
                            ui.label(RichText::new("Your conversation starts here.").color(MUTED));
                        }
                        if studio.turns.len() > 60 {
                            ui.label(
                                RichText::new("Showing the latest 60 messages.")
                                    .small()
                                    .color(MUTED),
                            );
                        }
                        for turn in studio
                            .turns
                            .iter()
                            .skip(studio.turns.len().saturating_sub(60))
                        {
                            let show_progress = studio
                                .library
                                .selected_character()
                                .is_some_and(|c| c.delegation.show_intermediate_text);
                            if matches!(turn.role.as_str(), "system" | "loop_instruction")
                                && turn.external.is_none()
                                || turn.role == "assistant_progress" && !show_progress
                            {
                                continue;
                            }
                            if turn.role == "tool_result" {
                                if show_progress {
                                    ui.collapsing("Tool result", |ui| {
                                        ui.label(&turn.content);
                                    });
                                }
                                continue;
                            }
                            ui.label(
                                RichText::new(conversation_turn_label(turn, name))
                                    .size(12.0)
                                    .strong()
                                    .color(
                                        if turn.role == "assistant" && turn.external.is_none() {
                                            ACCENT
                                        } else {
                                            MUTED
                                        },
                                    ),
                            );
                            let visible = if turn.role == "assistant_progress" {
                                turn.content.split("```waifu_tasks").next().unwrap_or("")
                            } else {
                                &turn.content
                            };
                            let text = ui.label(visible);
                            if ui.is_rect_visible(text.rect)
                                && turn.role == "assistant"
                                && turn.external.is_none()
                                && studio
                                    .turns
                                    .last()
                                    .is_some_and(|last| std::ptr::eq(last, turn))
                                && studio
                                    .completed_chat
                                    .as_ref()
                                    .is_some_and(|(_, reply)| reply.is_ok())
                                && let Some(timing) = &studio.reply_timing
                            {
                                timing.mark(waifu_studio::reply_timing::Stage::Layout);
                            }
                            ui.add_space(10.0);
                        }
                        let loop_turn = studio
                            .job
                            .as_ref()
                            .is_some_and(|job| studio.loop_chat_job == Some(job.id));
                        let show_progress = studio
                            .library
                            .selected_character()
                            .is_some_and(|c| c.delegation.show_intermediate_text);
                        if !studio.pending_response.is_empty() && (!loop_turn || show_progress) {
                            ui.label(RichText::new(name).small().strong().color(ACCENT));
                            let visible = if loop_turn {
                                studio
                                    .pending_response
                                    .split("```waifu_tasks")
                                    .next()
                                    .unwrap_or("")
                            } else {
                                &studio.pending_response
                            };
                            let text = ui.label(visible);
                            if ui.is_rect_visible(text.rect)
                                && let Some(timing) = &studio.reply_timing
                            {
                                timing.mark(waifu_studio::reply_timing::Stage::Layout);
                            }
                        }
                    });
            });
        });
}

fn composer(
    ctx: &egui::Context,
    studio: &mut Studio,
    memory: &mut waifu_studio::memory::MemoryState,
    semantic: &mut waifu_studio::semantic_memory::SemanticMemory,
    listening: &mut waifu_studio::listen::ListeningState,
    rail: f32,
    width: f32,
) {
    egui::Area::new("composer".into())
        .anchor(egui::Align2::CENTER_BOTTOM, EVec2::new(rail * 0.5, -24.0))
        .show(ctx, |ui| {
            ui.set_width((width - rail - 48.0).clamp(200.0, 980.0));
            if listening.is_busy() {
                microphone_shortcuts(ui, listening);
                ui.add_space(5.0);
            }
            egui::Frame::new()
                .fill(Color32::from_rgb(44, 44, 44))
                .stroke(Stroke::new(1.0, Color32::from_rgb(57, 57, 57)))
                .corner_radius(25)
                .inner_margin(14)
                .show(ui, |ui| {
                    if studio.attachment_drop_hovered {
                        ui.colored_label(ACCENT, "Drop images, PDF, DOCX, text or code files here");
                    }
                    if studio.has_attachments() {
                        ui.horizontal_wrapped(|ui| {
                            let mut remove = None;
                            for (index, image) in studio.attachments.iter().enumerate() {
                                if ui.small_button(format!("{} ×", image.name)).clicked() {
                                    remove = Some(index);
                                }
                            }
                            if let Some(index) = remove {
                                studio.attachments.remove(index);
                            }
                            let mut remove_document = None;
                            for (index, document) in studio.documents.iter().enumerate() {
                                if ui.small_button(format!("{} ×", document.name)).on_hover_text("Extracted text is sent with your next message; the original file is unchanged.").clicked() {
                                    remove_document = Some(index);
                                }
                            }
                            if let Some(index) = remove_document { studio.documents.remove(index); }
                            if studio.preparing_image() {
                                ui.spinner();
                            }
                        });
                        ui.add_space(5.0);
                    }
                    let response = egui::ScrollArea::vertical()
                        .id_salt("composer_text_scroll")
                        .max_height(126.0)
                        .show(ui, |ui| {
                            ui.add(
                                egui::TextEdit::multiline(&mut studio.composer)
                                    .id_salt("message_draft")
                                    .frame(egui::Frame::NONE)
                                    .desired_width(f32::INFINITY)
                                    .desired_rows(2)
                                    .char_limit(32_000)
                                    .font(egui::FontId::proportional(17.0))
                                    .return_key(egui::KeyboardShortcut::new(
                                        egui::Modifiers::SHIFT,
                                        egui::Key::Enter,
                                    ))
                                    .hint_text("Message your companion…"),
                            )
                        })
                        .inner;
                    let enter = response.has_focus()
                        && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
                    ui.add_space(10.0);
                    let busy = studio.chat_busy();
                    let mut send_clicked = false;
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        ui.style_mut().visuals.widgets.inactive.bg_fill = Color32::TRANSPARENT;
                        ui.style_mut().visuals.widgets.inactive.bg_stroke = Stroke::NONE;
                        ui.menu_button(RichText::new("+").size(29.0).color(TEXT), |ui| {
                            popup_layout(ui, 300.0);
                            if ui
                                .add_enabled(
                                    !studio.preparing_image() && studio.attachments.len() + studio.documents.len() < waifu_studio::attachments::MAX_FILES,
                                    egui::Button::new("Attach files…"),
                                )
                                .clicked()
                            {
                                studio.open_attachment();
                                ui.close();
                            }
                            if let Some(timing) = &studio.reply_timing {
                                ui.collapsing("Latest reply timing", |ui| {
                                    if let Some((stages, history, cancelled)) = timing.snapshot() {
                                        ui.small("Elapsed since Send; missing stages have not been observed.");
                                        for (stage, label) in waifu_studio::reply_timing::Stage::ALL {
                                            let value = stages[stage as usize]
                                                .map(|elapsed| format!("{} ms", elapsed.as_millis()))
                                                .unwrap_or_else(|| "—".into());
                                            ui.label(format!("{label}: {value}"));
                                        }
                                        if let Some(duration) = history {
                                            ui.label(format!("History save duration: {} ms", duration.as_millis()));
                                        }
                                        if cancelled { ui.small("Cancelled; late worker timings ignored."); }
                                        ui.small("Text layout is not GPU presentation or speaker onset. Local-only; not saved.");
                                    }
                                });
                            }
                            if ui.button("Import character…").clicked() {
                                studio.open_models();
                                ui.close();
                            }
                            if ui.button("Microphone & voice settings…").clicked() {
                                studio.tab = Tab::Voice;
                                ui.ctx().data_mut(|d| {
                                    d.insert_temp(egui::Id::new("voice_sections"), 3usize)
                                });
                                ui.close();
                            }
                            let history = if studio.show_history {
                                "Hide conversation history"
                            } else {
                                "Show conversation history"
                            };
                            if ui.button(history).clicked() {
                                studio.show_history = !studio.show_history;
                                studio.tab = Tab::Chat;
                                ui.close();
                            }
                        })
                        .response
                        .on_hover_text("Attachments and conversation actions");
                        let compact = ui.available_width() < 500.0;
                        composer_mode(ui, studio, compact);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if busy {
                                if composer_icon(ui, 4, true, "Stop generating reply").clicked() {
                                    studio.cancel_chat();
                                }
                            } else {
                                let enabled = (!studio.composer.trim().is_empty() || studio.has_attachments())
                                    && studio.library.selected.is_some()
                                    && !studio.preparing_image();
                                send_clicked = ui
                                    .add_enabled_ui(enabled, |ui| {
                                        composer_icon(
                                            ui,
                                            3,
                                            true,
                                            "Send · Enter (Shift+Enter for a new line)",
                                        )
                                    })
                                    .inner
                                    .clicked();
                            }
                            let microphone_hint = if !listening.config.enabled {
                                "Set up microphone transcription"
                            } else if listening.is_recording() {
                                "Stop recording and transcribe"
                            } else if listening.is_busy() {
                                "Cancel pending transcription"
                            } else {
                                "Start microphone transcription"
                            };
                            if composer_icon(ui, 2, listening.is_busy(), microphone_hint).clicked()
                            {
                                if !listening.config.enabled {
                                    studio.tab = Tab::Voice;
                                    ui.ctx().data_mut(|d| {
                                        d.insert_temp(egui::Id::new("voice_sections"), 3usize)
                                    });
                                } else if listening.is_recording() {
                                    listening.stop();
                                } else if listening.is_busy() {
                                    listening.cancel();
                                } else {
                                    listening.start_for(
                                        studio.library.selected,
                                        studio.composer.trim().is_empty(),
                                    );
                                }
                            }
                            ui.add_enabled_ui(!busy, |ui| composer_connection(ui, studio, compact));
                            if busy {
                                ui.spinner();
                            }
                        });
                    });
                    if !busy && (enter || send_clicked) && !studio.preparing_image() {
                        studio.send_chat_with_semantic(memory, Some(semantic));
                        response.request_focus();
                    }
                });
        });
}

fn composer_mode(ui: &mut egui::Ui, studio: &mut Studio, compact: bool) {
    let mut enabled = studio
        .library
        .selected_character()
        .is_some_and(|c| c.delegation.tools.enabled);
    let was_enabled = enabled;
    let label = if compact {
        "Mode"
    } else if enabled {
        "Agent loop on"
    } else {
        "Agent loop off"
    };
    ui.menu_button(RichText::new(label).size(14.0).color(if enabled { Color32::from_rgb(255, 145, 80) } else { MUTED }), |ui| {
        popup_layout(ui, 290.0);
        ui.label("Companion behavior");
        ui.add_enabled_ui(studio.library.selected.is_some(), |ui| {
            ui.selectable_value(&mut enabled, false, "Normal chat · loop off");
            ui.selectable_value(&mut enabled, true, "Agent loop · use enabled tools");
        });
        kit::note(ui, "Off by default. Search and tool round trips add latency. Choose allowed tools in Assistant tools.");
        if ui.button("Choose search, MCP & delegation tools…").clicked() { studio.tab = Tab::Tools; ui.close(); }
        if ui.button("Open reviewed coding tasks…").clicked() { studio.tab = Tab::Tasks; ui.close(); }
    });
    if enabled != was_enabled {
        if let Some(character) = studio
            .library
            .characters
            .iter_mut()
            .find(|c| Some(c.id) == studio.library.selected)
        {
            character.delegation.tools.enabled = enabled;
        }
        studio.save();
    }
}

/// Search and virtualized results live inside the selector, leaving the page compact.
fn model_dropdown(
    ui: &mut egui::Ui,
    salt: &str,
    models: &[String],
    current: &str,
) -> Option<String> {
    let mut picked = None;
    let width = control_width(ui);
    egui::ComboBox::from_id_salt((salt, "dropdown"))
        .height(400.0)
        .truncate()
        .selected_text(if current.is_empty() {
            "Choose a model…"
        } else {
            current
        })
        .width(width)
        .show_ui(ui, |ui| {
            popup_layout(ui, 380.0);
            picked = model_list(ui, salt, models, current);
            ui.collapsing("Advanced", |ui| {
                if let Some(custom) = custom_model_input(ui, salt, current) {
                    picked = Some(custom);
                }
            });
            if picked.is_some() {
                ui.close();
            }
        })
        .response
        .on_hover_text(current);
    picked
}

/// Keep exact IDs available without filling the main picker with technical fields.
fn custom_model_input(ui: &mut egui::Ui, salt: &str, current: &str) -> Option<String> {
    let id = ui.make_persistent_id((salt, "custom-model"));
    let selection_id = id.with("selection");
    let mut draft = ui
        .data(|data| data.get_temp::<String>(id))
        .unwrap_or_else(|| current.to_owned());
    if ui
        .data(|data| data.get_temp::<String>(selection_id))
        .as_deref()
        != Some(current)
    {
        draft = current.to_owned();
    }
    let mut picked = None;
    ui.label("Exact model ID");
    ui.horizontal_wrapped(|ui| {
        let response = ui.add(
            egui::TextEdit::singleline(&mut draft)
                .desired_width((ui.available_width() - 52.0).clamp(1.0, 280.0))
                .hint_text("provider/model")
                .char_limit(256),
        );
        let enter = response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
        let use_model = ui.add_enabled(!draft.trim().is_empty(), egui::Button::new("Use"));
        if (use_model.clicked() || enter) && !draft.trim().is_empty() {
            picked = Some(draft.trim().to_owned());
        }
    });
    ui.data_mut(|data| {
        data.insert_temp(id, picked.clone().unwrap_or(draft));
        data.insert_temp(selection_id, current.to_owned());
    });
    picked
}

/// Virtual rows keep large catalogs responsive and make every matching model reachable.
fn model_list(ui: &mut egui::Ui, salt: &str, models: &[String], current: &str) -> Option<String> {
    let id = ui.make_persistent_id((salt, "search"));
    let mut search = ui
        .ctx()
        .data_mut(|d| d.get_temp::<String>(id).unwrap_or_default());
    ui.add(
        egui::TextEdit::singleline(&mut search)
            .desired_width(f32::INFINITY)
            .hint_text("Search model or provider…"),
    );
    ui.ctx().data_mut(|d| d.insert_temp(id, search.clone()));
    let words: Vec<String> = search.split_whitespace().map(str::to_lowercase).collect();
    let filtered: Vec<_> = models
        .iter()
        .filter(|model| {
            let lower = model.to_lowercase();
            words.iter().all(|word| lower.contains(word))
        })
        .collect();
    ui.label(
        RichText::new(format!(
            "{} matching · {} available",
            filtered.len(),
            models.len()
        ))
        .small()
        .color(MUTED),
    );
    let mut picked = None;
    egui::ScrollArea::vertical()
        .id_salt((salt, "rows"))
        .max_height(260.0)
        .show_rows(ui, 28.0, filtered.len(), |ui, rows| {
            for row in rows {
                let model = filtered[row];
                if ui
                    .add_sized(
                        [ui.available_width(), 28.0],
                        egui::Button::selectable(current == model, model).truncate(),
                    )
                    .on_hover_text(model)
                    .clicked()
                {
                    picked = Some(model.clone());
                }
            }
        });
    if filtered.is_empty() {
        ui.small("No matches. Use Advanced to enter an exact model ID.");
    }
    picked
}

pub(crate) fn speech_model_picker(
    ui: &mut egui::Ui,
    salt: &str,
    catalog: &waifu_studio::model_catalog::ModelCatalog,
    model: &mut String,
) {
    ui.push_id(salt, |ui| {
        if let Some(selected) = kit::row(
            ui,
            "Model",
            "Search available models or enter an exact ID.",
            |ui| model_dropdown(ui, salt, &catalog.models, model),
        ) {
            *model = selected;
        }
        ui.horizontal_wrapped(|ui| {
            if catalog.loading {
                ui.spinner();
            }
            ui.label(RichText::new(&catalog.status).small().color(MUTED));
        });
    });
}

/// Full memory inspectors and connection settings, behind the Memory page's Advanced fold.
fn memory_advanced(
    ui: &mut egui::Ui,
    studio: &mut Studio,
    memory: &mut waifu_studio::memory::MemoryState,
    semantic: &mut waifu_studio::semantic_memory::SemanticMemory,
) {
    group(ui, "Local notes", |ui| {
        crate::memory_controls::ui(ui, memory)
    });
    group(ui, "Facts, preferences & evidence", |ui| {
        if ui
            .add_enabled(
                !studio.facts.reflection.running()
                    && !studio.facts.reflection.shutting_down(),
                egui::Button::new("Copy chat connection for reflection drafts"),
            )
            .on_hover_text("Copies the endpoint, model and key source. Provider routing and BYOK overrides are not copied. Review the reflection connection before requesting a draft.")
            .clicked()
            && let Some(provider) = studio.library.providers.get(studio.library.provider)
        {
            match waifu_studio::facts_memory::reflection::ReflectionConfig::from_provider(provider) {
                Ok(config) => studio.facts.reflection.config = config,
                Err(error) => studio.error = Some(error),
            }
        }
        crate::facts_memory_controls::ui(ui, &mut studio.facts);
        ui.separator();
        ui.weak(&studio.facts.triage_status);
        ui.separator();
        kit::section(ui, "Automatic reflection drafts");
        kit::note(
            ui,
            "Drafts use up to 12 accepted records, without transcripts or earlier reflections. Review each draft before recall. Runs for the selected companion while the app is open.",
        );
        let before = studio.library.scene.automatic_reflection.clone();
        let reflection_connection = studio.facts.reflection.config.clone();
        let automatic = &mut studio.library.scene.automatic_reflection;
        kit::slider_row(
            ui,
            "New accepted records per draft",
            "",
            egui::Slider::new(&mut automatic.new_records, 1..=12),
        );
        kit::slider_row(
            ui,
            "Minimum minutes between attempts",
            "",
            egui::Slider::new(&mut automatic.cooldown_minutes, 10..=1440),
        );
        ui.label(format!(
            "Connection: {} · {}",
            automatic.connection.endpoint, automatic.connection.model
        ));
        if ui.button("Use reflection connection").clicked() {
            automatic.connection = reflection_connection;
        }
        kit::note(
            ui,
            "Waits while a draft needs review. Saved per companion, excluding session keys. Changing settings cancels active drafts.",
        );
        ui.weak(&studio.facts.automatic.status);
        if studio.library.scene.automatic_reflection != before {
            studio.save();
        }
    });
    ui.separator();
    let semantic_source = waifu_studio::semantic_memory::MemorySources {
        scope: semantic.scope(),
        memory,
        facts: &studio.facts,
    };
    kit::note(
        ui,
        "Search the selected memory source for this draft. A matching result is attached only to the next request.",
    );
    if ui
        .add_enabled(
            !studio.composer.trim().is_empty()
                && semantic.ready(studio.library.selected, &semantic_source),
            egui::Button::new("Find memories for current draft"),
        )
        .clicked()
    {
        semantic.query(studio.library.selected, &semantic_source, &studio.composer);
    }
    if semantic.has_result_for(studio.library.selected, &semantic_source, &studio.composer) {
        kit::note(
            ui,
            "Retrieved memories will be used once when this draft is sent; they are not saved in chat.",
        );
    }
}

/// Dropdown sized for the right side of a settings row; status shows on hover.
fn decision_model_compact(
    ui: &mut egui::Ui,
    salt: &str,
    catalog: &mut waifu_studio::model_catalog::ModelCatalog,
    connection: &waifu_studio::recipient_decision::DecisionConfig,
    model: &mut String,
) {
    use waifu_studio::model_catalog::CatalogKind::Decision;
    catalog.sync(
        &connection.endpoint,
        &connection.key_env,
        &connection.session_key,
        Decision,
    );
    ui.horizontal_wrapped(|ui| {
        if let Some(picked) = model_dropdown(ui, salt, &catalog.models, model) {
            *model = picked;
        }
        if catalog.loading {
            ui.spinner();
        } else if catalog.models.is_empty()
            && ui
                .small_button("Retry")
                .on_hover_text(&catalog.status)
                .clicked()
        {
            catalog.retry(
                &connection.endpoint,
                &connection.key_env,
                &connection.session_key,
                Decision,
            );
        }
    });
}

fn model_discovery_status(ui: &mut egui::Ui, studio: &Studio) {
    let (loading, error) = studio.models_status();
    if loading {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.small("Loading this connection’s models…");
        });
    } else if let Some(error) = error {
        ui.label(RichText::new(error).small().color(ACCENT));
    }
}

fn composer_connection(ui: &mut egui::Ui, studio: &mut Studio, compact: bool) {
    use waifu_studio::provider::ReasoningEffort;
    let current = studio.library.providers.get(studio.library.provider);
    let model = current
        .map(|p| p.model.as_str())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("Choose model");
    let limit =
        (((ui.available_width() - 65.0) / 8.0) as usize).clamp(6, if compact { 18 } else { 36 });
    let short: String = if model.chars().count() > limit {
        model.chars().take(limit - 1).chain("…".chars()).collect()
    } else {
        model.to_owned()
    };
    let effort = current
        .and_then(|p| p.reasoning_effort)
        .map(|e| format!(" {}", e.label()))
        .unwrap_or_default();
    let label = format!("{short}{effort}     ");
    let mut changed = false;
    let mut fetch = false;
    let open_id = ui.make_persistent_id("composer-connection-open");
    let mut open = ui
        .ctx()
        .data(|d| d.get_temp::<bool>(open_id))
        .unwrap_or(false);
    let menu = ui.button(RichText::new(label).size(15.0).color(TEXT));
    if menu.clicked() {
        open = !open;
    }
    egui::Popup::from_response(&menu)
        .open_bool(&mut open)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            popup_layout(ui, 400.0);
            ui.with_layout(egui::Layout::top_down(egui::Align::LEFT), |ui| {
                ui.strong("Model & connection");
                let previous = studio.library.provider;
                let selected_name = studio
                    .library
                    .providers
                    .get(previous)
                    .map(|p| p.name.as_str())
                    .unwrap_or("Choose connection");
                egui::ComboBox::from_id_salt("composer_provider")
                    .selected_text(selected_name)
                    .truncate()
                    .width(ui.available_width())
                    .show_ui(ui, |ui| {
                        for (index, provider) in studio.library.providers.iter().enumerate() {
                            ui.selectable_value(
                                &mut studio.library.provider,
                                index,
                                &provider.name,
                            );
                        }
                    });
                changed |= previous != studio.library.provider;
                let index = studio.library.provider;
                studio.ensure_models();
                let mut picked = model_list(
                    ui,
                    "composer_models",
                    studio
                        .model_lists
                        .get(&index)
                        .map(Vec::as_slice)
                        .unwrap_or_default(),
                    studio
                        .library
                        .providers
                        .get(index)
                        .map(|p| p.model.as_str())
                        .unwrap_or_default(),
                );
                model_discovery_status(ui, studio);
                if ui
                    .add_enabled(
                        !studio.models_status().0,
                        egui::Button::new("Refresh available models"),
                    )
                    .clicked()
                {
                    fetch = true;
                }
                ui.separator();
                if let Some(provider) = studio.library.providers.get_mut(index) {
                    ui.push_id(index, |ui| {
                        ui.collapsing("Advanced", |ui| {
                            if let Some(custom) =
                                custom_model_input(ui, "composer", &provider.model)
                            {
                                picked = Some(custom);
                            }
                        });
                    });
                    if let Some(model) = picked {
                        if provider.model != model {
                            provider.reasoning_effort = None;
                        }
                        provider.model = model;
                        changed = true;
                    }
                    let old_effort = provider.reasoning_effort;
                    let supported = provider.supported_reasoning_efforts();
                    kit::row(
                        ui,
                        "Reasoning effort",
                        "Auto uses the model default.",
                        |ui| {
                            egui::ComboBox::from_id_salt("composer-reasoning-effort")
                                .width(ui.available_width().min(160.0))
                                .truncate()
                                .selected_text(
                                    provider
                                        .reasoning_effort
                                        .map_or("Auto", ReasoningEffort::label),
                                )
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(
                                        &mut provider.reasoning_effort,
                                        None,
                                        "Auto",
                                    );
                                    for effort in ReasoningEffort::ALL {
                                        if supported
                                            .as_ref()
                                            .is_some_and(|listed| !listed.contains(&effort))
                                        {
                                            continue;
                                        }
                                        ui.selectable_value(
                                            &mut provider.reasoning_effort,
                                            Some(effort),
                                            effort.label(),
                                        );
                                    }
                                });
                        },
                    );
                    if let Some(error) = provider.reasoning_effort_error() {
                        ui.colored_label(egui::Color32::LIGHT_RED, error);
                    }
                    changed |= old_effort != provider.reasoning_effort;
                }
                ui.separator();
                if ui.button("Manage connections & API keys…").clicked() {
                    studio.tab = Tab::Connections;
                    ui.close();
                }
            });
        });
    ui.ctx().data_mut(|d| d.insert_temp(open_id, open));
    let menu = menu.on_hover_text("Choose connection, model and reasoning effort");
    let arrow = egui::pos2(menu.rect.right() - 11.0, menu.rect.center().y);
    ui.painter().add(egui::Shape::line(
        vec![
            arrow + EVec2::new(-3.5, -1.5),
            arrow + EVec2::new(0.0, 2.0),
            arrow + EVec2::new(3.5, -1.5),
        ],
        Stroke::new(1.3, MUTED),
    ));
    if changed {
        studio.connected = false;
        studio.save();
    }
    if fetch {
        studio.refresh_models();
    }
}

/// Show recording actions only while a capture/transcription is active.
fn microphone_shortcuts(ui: &mut egui::Ui, listening: &mut waifu_studio::listen::ListeningState) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().button_padding = EVec2::new(8.0, 4.0);
        let label = if listening.is_recording() {
            "Recording · auto-transcribes after silence"
        } else {
            "Preparing transcript…"
        };
        ui.label(
            RichText::new(label)
                .small()
                .color(if listening.is_recording() {
                    ACCENT
                } else {
                    MUTED
                }),
        );
        if listening.is_recording() && ui.small_button("Stop + transcribe").clicked() {
            listening.stop();
        }
        if ui.small_button("Cancel").clicked() {
            listening.cancel();
        }
    });
    ui.label(RichText::new(&listening.status).size(10.0).color(MUTED));
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_millis(100));
}

/// Painted icons avoid font-dependent glyphs in the compact input row.
fn composer_icon(ui: &mut egui::Ui, icon: u8, active: bool, label: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(EVec2::splat(36.0), egui::Sense::click());
    let painter = ui.painter();
    if active || response.hovered() {
        painter.circle_filled(
            rect.center(),
            17.0,
            if active {
                TEXT
            } else {
                Color32::from_rgb(52, 52, 52)
            },
        );
    }
    let color = if !ui.is_enabled() {
        Color32::from_gray(90)
    } else if active {
        SURFACE
    } else {
        MUTED
    };
    let stroke = Stroke::new(1.8, color);
    let p = |x, y| rect.center() + EVec2::new(x, y);
    match icon {
        0 => {
            painter.line_segment([p(-8.0, 0.0), p(8.0, 0.0)], stroke);
            painter.line_segment([p(0.0, -8.0), p(0.0, 8.0)], stroke);
        }
        1 => {
            painter.rect_stroke(
                egui::Rect::from_center_size(p(0.0, 1.0), EVec2::new(21.0, 15.0)),
                3,
                stroke,
                egui::StrokeKind::Middle,
            );
            painter.circle_stroke(p(0.0, 1.0), 4.0, stroke);
            painter.line_segment([p(-5.0, -7.0), p(-3.0, -10.0)], stroke);
            painter.line_segment([p(-3.0, -10.0), p(3.0, -10.0)], stroke);
            painter.line_segment([p(3.0, -10.0), p(5.0, -7.0)], stroke);
        }
        2 => {
            painter.rect_stroke(
                egui::Rect::from_center_size(p(0.0, -3.0), EVec2::new(7.0, 13.0)),
                4,
                stroke,
                egui::StrokeKind::Middle,
            );
            painter.add(egui::Shape::line(
                vec![
                    p(-7.0, -1.0),
                    p(-7.0, 3.0),
                    p(-4.0, 7.0),
                    p(4.0, 7.0),
                    p(7.0, 3.0),
                    p(7.0, -1.0),
                ],
                stroke,
            ));
            painter.line_segment([p(0.0, 7.0), p(0.0, 11.0)], stroke);
            painter.line_segment([p(-4.0, 11.0), p(4.0, 11.0)], stroke);
        }
        3 => {
            painter.line_segment([p(0.0, 9.0), p(0.0, -9.0)], stroke);
            painter.line_segment([p(-7.0, -2.0), p(0.0, -9.0)], stroke);
            painter.line_segment([p(0.0, -9.0), p(7.0, -2.0)], stroke);
        }
        _ => {
            painter.rect_filled(
                egui::Rect::from_center_size(rect.center(), EVec2::splat(12.0)),
                2,
                color,
            );
        }
    }
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    response.on_hover_text(label)
}

fn nav_button(ui: &mut egui::Ui, icon: u8, selected: bool, label: &str) -> egui::Response {
    let expanded = ui.available_width() > 110.0;
    let (rect, response) = ui.allocate_exact_size(
        EVec2::new(
            if expanded { ui.available_width() } else { 44.0 },
            if expanded { 32.0 } else { 40.0 },
        ),
        egui::Sense::click(),
    );
    let painter = ui.painter();
    if selected || response.hovered() {
        painter.rect_filled(
            rect,
            14,
            if selected {
                Color32::from_rgb(54, 43, 41)
            } else {
                Color32::from_rgb(43, 43, 43)
            },
        );
    }
    let center = if expanded {
        egui::pos2(rect.left() + 22.0, rect.center().y)
    } else {
        rect.center()
    };
    if expanded {
        painter.text(
            egui::pos2(rect.left() + 46.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::proportional(13.0),
            if selected { TEXT } else { MUTED },
        );
    }
    let color = if selected { ACCENT } else { MUTED };
    let stroke = Stroke::new(1.7, color);
    let p = |x, y| center + EVec2::new(x, y);
    match icon {
        0 => {
            painter.rect_stroke(
                egui::Rect::from_center_size(center, EVec2::new(17.0, 14.0)),
                5,
                stroke,
                egui::StrokeKind::Middle,
            );
            painter.line_segment([p(-6.0, 6.0), p(-9.0, 10.0)], stroke);
            painter.line_segment([p(-4.0, -2.0), p(4.0, -2.0)], stroke);
            painter.line_segment([p(-4.0, 2.0), p(1.0, 2.0)], stroke);
        }
        1 => {
            painter.rect_stroke(
                egui::Rect::from_center_size(center, EVec2::new(16.0, 20.0)),
                4,
                stroke,
                egui::StrokeKind::Middle,
            );
            painter.circle_stroke(p(0.0, -3.0), 3.0, stroke);
            painter.line_segment([p(-4.0, 6.0), p(-3.0, 2.0)], stroke);
            painter.line_segment([p(-3.0, 2.0), p(3.0, 2.0)], stroke);
            painter.line_segment([p(3.0, 2.0), p(4.0, 6.0)], stroke);
        }
        2 => {
            let points = [
                p(0.0, -10.0),
                p(9.0, -5.0),
                p(9.0, 5.0),
                p(0.0, 10.0),
                p(-9.0, 5.0),
                p(-9.0, -5.0),
            ];
            painter.add(egui::Shape::closed_line(points.to_vec(), stroke));
            painter.line_segment([p(-9.0, -5.0), p(0.0, 0.0)], stroke);
            painter.line_segment([p(9.0, -5.0), p(0.0, 0.0)], stroke);
            painter.line_segment([p(0.0, 0.0), p(0.0, 10.0)], stroke);
        }
        3 => {
            painter.circle_stroke(center, 5.0, stroke);
            for i in 0..8 {
                let angle = i as f32 * std::f32::consts::FRAC_PI_4;
                let v = EVec2::new(angle.cos(), angle.sin());
                painter.line_segment([center + v * 7.0, center + v * 10.0], stroke);
            }
        }
        6 => {
            painter.rect_stroke(
                egui::Rect::from_center_size(p(0.0, -3.0), EVec2::new(8.0, 14.0)),
                4,
                stroke,
                egui::StrokeKind::Middle,
            );
            painter.line_segment([p(-8.0, 0.0), p(-8.0, 6.0)], stroke);
            painter.line_segment([p(-8.0, 6.0), p(8.0, 6.0)], stroke);
            painter.line_segment([p(8.0, 6.0), p(8.0, 0.0)], stroke);
            painter.line_segment([p(0.0, 6.0), p(0.0, 11.0)], stroke);
        }
        7 => {
            for x in [-7.0, 0.0, 7.0] {
                painter.rect_stroke(
                    egui::Rect::from_center_size(p(x, 0.0), EVec2::new(5.0, 20.0)),
                    1,
                    stroke,
                    egui::StrokeKind::Middle,
                );
            }
        }
        9 => {
            painter.rect_stroke(
                egui::Rect::from_center_size(center, EVec2::new(24.0, 19.0)),
                3,
                stroke,
                egui::StrokeKind::Middle,
            );
            painter.line_segment([p(-7.0, -4.0), p(-3.0, 0.0)], stroke);
            painter.line_segment([p(-3.0, 0.0), p(-7.0, 4.0)], stroke);
            painter.line_segment([p(1.0, 4.0), p(7.0, 4.0)], stroke);
        }
        8 => {
            for (x, y) in [(-8.0, -7.0), (8.0, -7.0), (0.0, 9.0)] {
                painter.line_segment([center, p(x, y)], stroke);
                painter.circle_stroke(p(x, y), 3.0, stroke);
            }
        }
        4 => {
            painter.rect_stroke(
                egui::Rect::from_center_size(center, EVec2::new(21.0, 15.0)),
                3,
                stroke,
                egui::StrokeKind::Middle,
            );
            for y in [-3.0, 2.0] {
                painter.line_segment([p(-6.0, y), p(6.0, y)], stroke);
            }
        }
        _ => {
            painter.rect_stroke(
                egui::Rect::from_center_size(p(0.0, -2.0), EVec2::new(22.0, 15.0)),
                3,
                stroke,
                egui::StrokeKind::Middle,
            );
            painter.line_segment([p(0.0, 6.0), p(0.0, 10.0)], stroke);
            painter.line_segment([p(-6.0, 10.0), p(6.0, 10.0)], stroke);
        }
    }
    response.on_hover_text(label)
}

fn agent_tasks(
    ui: &mut egui::Ui,
    studio: &mut Studio,
    agents: &mut waifu_studio::agents::AgentState,
    board: &mut waifu_studio::task_board::TaskBoard,
) {
    use waifu_studio::{agents::AgentProvider, delegation::Provider};
    let mut staged = None;
    for (index, proposal) in studio.proposals.iter().enumerate() {
        ui.group(|ui| {
            ui.strong(&proposal.title);
            ui.small(format!("Suggested worker: {:?}", proposal.provider));
            if ui.button("Add to parallel task board").clicked()
                && let Err(error) = board.enqueue_proposal(
                    proposal,
                    waifu_studio::task_board::TaskConfig::snapshot(agents),
                )
            {
                studio.error = Some(error);
            }
            ui.collapsing("Task prompt", |ui| {
                ui.label(&proposal.prompt);
            });
            if ui
                .add_enabled(!agents.is_busy(), egui::Button::new("Stage for review"))
                .clicked()
            {
                staged = Some(index);
            }
        });
    }
    if let Some(index) = staged {
        let proposal = studio.proposals.remove(index);
        agents.provider = match proposal.provider {
            Provider::Codex => AgentProvider::Codex,
            Provider::Claude => AgentProvider::Claude,
            Provider::Pi => AgentProvider::Pi,
            Provider::Custom => AgentProvider::Custom,
        };
        agents.prompt = proposal.prompt;
        agents.disarm();
    }
    ui.separator();
    group(ui, "Worker settings", |ui| {
        crate::agent_controls::ui(ui, agents)
    });
    if ui
        .add_enabled(
            !agents.is_busy() && !agents.output.is_empty(),
            egui::Button::new("Add result to this conversation"),
        )
        .clicked()
        && let Some(mut context) = agents.take_context()
    {
        let limit = 28_000;
        if context.len() > limit {
            let mut end = limit;
            while !context.is_char_boundary(end) {
                end -= 1;
            }
            context.truncate(end);
            context.push_str("\n[Result excerpt truncated by the companion]");
        }
        if studio.composer.len() + context.len() > 32_000 {
            studio.error = Some("Clear or shorten the composer before adding this result.".into());
        } else {
            if !studio.composer.is_empty() {
                studio.composer.push('\n');
            }
            studio.composer.push_str(&context);
            studio.tab = Tab::Chat;
            studio.notice =
                Some("Agent result added. Review and send it to your companion.".into());
        }
    }
}

fn append_context(studio: &mut Studio, context: &str) {
    if studio.composer.len() + context.len() + 1 > 32_000 {
        studio.error = Some("Shorten the draft before adding this result (32 KB limit).".into());
    } else {
        if !studio.composer.is_empty() {
            studio.composer.push('\n');
        }
        studio.composer.push_str(context);
        studio.notice = Some("Result added to draft. Review it before sending.".into());
        studio.tab = Tab::Chat;
    }
}

#[cfg(test)]
mod composer_tests {
    use super::*;

    #[test]
    fn external_history_labels_never_fall_back_to_character_or_user_attribution() {
        use waifu_studio::{
            data::{ExternalMessage, Turn},
            extension_protocol::IntegrationOrigin,
        };
        let mut turn = Turn::external(
            "I like tea",
            ExternalMessage {
                message_id: uuid::Uuid::new_v4(),
                origin: IntegrationOrigin {
                    integration_id: "org.example.weather".into(),
                    display_name: "Weather".into(),
                },
            },
        )
        .unwrap();
        assert_eq!(
            conversation_turn_label(&turn, "Alice"),
            "External · Weather"
        );
        turn.role = "assistant".into();
        assert_eq!(
            conversation_turn_label(&turn, "Alice"),
            "External · Unattributed"
        );
        turn.role = "external".into();
        turn.external.as_mut().unwrap().message_id = uuid::Uuid::nil();
        assert_eq!(
            conversation_turn_label(&turn, "Alice"),
            "External · Unattributed"
        );
        turn.external = None;
        assert_eq!(
            conversation_turn_label(&turn, "Alice"),
            "External · Unattributed"
        );
        turn.role = "user".into();
        assert_eq!(conversation_turn_label(&turn, "Alice"), "You");
        turn.role = "assistant".into();
        assert_eq!(conversation_turn_label(&turn, "Alice"), "Alice");
    }

    #[test]
    fn ui_motion_entries_preserve_host_pack_catalog_despite_empty_legacy_caches() {
        let mut fixture = crate::motion_services::tests::Headless::new();
        fixture.install_starter();
        let services = fixture.app.world().resource::<MotionServices>().clone();
        let expected: Vec<_> = services
            .extension_entries()
            .iter()
            .map(|entry| entry.path.clone())
            .collect();
        assert_eq!(expected.len(), 3);
        let ctx = egui::Context::default();
        bind_motion_services(&ctx, &services);
        let entries = motion_entries(&ctx, false);
        assert!(
            expected
                .iter()
                .all(|path| entries.iter().any(|entry| &entry.path == path))
        );
        // Old page caches are deliberately empty/stale. Neither cached lookup
        // nor an explicit UI rescan may replace the host's extension catalog.
        ctx.data_mut(|data| {
            data.insert_temp(
                egui::Id::new("extension-motion-entries"),
                motion_services::MotionEntries::default(),
            );
            data.insert_temp(
                egui::Id::new("motion_library_entries"),
                motion_services::MotionEntries::default(),
            );
        });
        for rescan in [false, true] {
            let entries = motion_entries(&ctx, rescan);
            assert!(
                expected
                    .iter()
                    .all(|path| entries.iter().any(|entry| &entry.path == path))
            );
            assert_eq!(services.extension_entries().len(), 3);
        }
        let id = fixture
            .app
            .world()
            .resource::<waifu_studio::extensions::ExtensionState>()
            .packs[0]
            .installed
            .installation_id;
        fixture
            .app
            .world_mut()
            .resource_mut::<waifu_studio::extensions::ExtensionState>()
            .set_enabled(id, false);
        fixture.settle_extensions();
        let entries = motion_entries(&ctx, false);
        assert!(entries.iter().all(|entry| !expected.contains(&entry.path)));
        assert!(services.extension_entries().is_empty());
    }

    #[test]
    fn owner_switch_regression_discards_old_automatic_reaction_but_preserves_manual_hold() {
        use std::sync::Arc;
        use waifu_studio::{
            animation::parse_bvh,
            data::{ReplyMotionMode, Store},
        };
        let directory = std::env::temp_dir().join(format!("motion-owner-{}", uuid::Uuid::new_v4()));
        let (store, mut library) = Store::open(directory.clone()).unwrap();
        let first = uuid::Uuid::new_v4();
        library.selected = Some(first);
        library.scene.reply_motion.mode = ReplyMotionMode::Off;
        let mut studio = Studio::new(store, library);
        let ctx = egui::Context::default();
        lock(&companion_shared(&ctx)).select_owner(Some((first, 1)));
        let clip = Arc::new(
            parse_bvh(include_str!(
                "../assets/animations/companion/idle-look-down.bvh"
            ))
            .unwrap(),
        );
        let mut animation = AnimationState::default();
        animation.select_one_shot(clip.clone(), Some("old-reaction.bvh".into()));
        studio.library.selected = Some(uuid::Uuid::new_v4());
        reply_motion_frame(&ctx, &mut studio, &mut animation, Some(2), false);
        assert!(
            !animation.gesture_active() && !animation.playing,
            "old character reaction must not restart on the new avatar"
        );
        animation.select_one_shot(clip.clone(), Some("reload-reaction.bvh".into()));
        reply_motion_frame(&ctx, &mut studio, &mut animation, Some(3), false);
        assert!(
            !animation.gesture_active(),
            "same character, new avatar lifetime"
        );
        animation.select_clip(clip, Some("manual.bvh".into()));
        animation.take_manual_control();
        animation.pause();
        reply_motion_frame(&ctx, &mut studio, &mut animation, Some(4), false);
        assert!(animation.manual_controlled() && animation.is_posing() && !animation.playing);
        drop(studio);
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn resume_regression_restarts_the_same_single_idle_after_manual_pause() {
        use std::sync::Arc;
        use waifu_studio::{animation::parse_bvh, data::Store};
        let directory =
            std::env::temp_dir().join(format!("motion-resume-{}", uuid::Uuid::new_v4()));
        let (store, mut library) = Store::open(directory.clone()).unwrap();
        let character = uuid::Uuid::new_v4();
        library.selected = Some(character);
        let studio = Studio::new(store, library);
        let ctx = egui::Context::default();
        let path = std::path::PathBuf::from("idle-one.bvh");
        let clip = Arc::new(
            parse_bvh(include_str!(
                "../assets/animations/companion/idle-look-down.bvh"
            ))
            .unwrap(),
        );
        ctx.data_mut(|data| {
            data.insert_temp(
                egui::Id::new("motion_library_entries"),
                Arc::new(vec![motion_library::MotionEntry {
                    metadata: None,
                    group: "fixture".into(),
                    label: "idle one".into(),
                    path: path.clone(),
                    format: "BVH",
                }]),
            )
        });
        let shared = companion_shared(&ctx);
        {
            let mut controller = lock(&shared);
            controller.select_owner(Some((character, 1)));
            controller.cache.push_back((path.clone(), clip.clone()));
        }
        let mut animation = AnimationState::default();
        animation.select_clip(clip, Some(path.clone()));
        manual_motion(&ctx, &mut animation);
        animation.pause();
        animation.resume_automatic_control();
        lock(&shared).resume();
        companion_motion_frame(&ctx, &studio, &mut animation, Some(1), false);
        assert!(
            animation.playing,
            "Resume must restart even when the chosen path is unchanged"
        );
        assert_eq!(animation.path, Some(path));
        drop(studio);
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn real_animation_reaction_hands_off_to_current_speech_role() {
        use std::{path::PathBuf, sync::Arc, time::Duration};
        use waifu_studio::{
            animation::{BvhAnimationPlugin, parse_bvh},
            data::Store,
        };
        const CLIP: &str = "HIERARCHY\nROOT Hips\n{\nOFFSET 0 1 0\nCHANNELS 6 Xposition Yposition Zposition Zrotation Xrotation Yrotation\nEnd Site\n{\nOFFSET 0 1 0\n}\n}\nMOTION\nFrames: 2\nFrame Time: 0.5\n0 0 0 0 0 0\n0 0 0 10 0 0\n";
        #[derive(Resource)]
        struct Fixture {
            playing: bool,
        }
        let root_dir =
            std::env::temp_dir().join(format!("reaction-handoff-{}", uuid::Uuid::new_v4()));
        let (store, mut library) = Store::open(root_dir.clone()).unwrap();
        let character = uuid::Uuid::new_v4();
        library.selected = Some(character);
        let mut studio = Studio::new(store, library);
        studio.speech_character = Some(character);
        let services = MotionServices::default();
        let entries = Arc::new(
            ["idle-one.bvh", "talking-one.bvh"]
                .into_iter()
                .map(|name| motion_library::MotionEntry {
                    metadata: None,
                    group: "fixture".into(),
                    label: name.into(),
                    path: name.into(),
                    format: "BVH",
                })
                .collect::<Vec<_>>(),
        );
        services.replace_entries(entries);
        let clip = Arc::new(parse_bvh(CLIP).unwrap());
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
                Duration::from_millis(100),
            ))
            .insert_resource(studio)
            .insert_resource(services.clone())
            .insert_resource(Fixture { playing: true })
            .init_resource::<AvatarLoadState>()
            .add_plugins(BvhAnimationPlugin)
            .add_systems(
                PostUpdate,
                |fixture: Res<Fixture>,
                 motions: Res<MotionServices>,
                 studio: Res<Studio>,
                 avatar: Res<AvatarLoadState>,
                 mut animation: ResMut<AnimationState>| {
                    motion_services::companion_motion_frame(
                        &motions,
                        &studio,
                        &mut animation,
                        avatar.active.map(Entity::to_bits),
                        fixture.playing,
                    );
                },
            );
        let root = app.world_mut().spawn(Transform::default()).id();
        app.world_mut().spawn((
            serde_vrm::vrm1::vrmc_vrm::humanoid::HumanBoneName::Hips,
            Transform::from_xyz(0., 1., 0.),
            ChildOf(root),
        ));
        app.world_mut().resource_mut::<AvatarLoadState>().active = Some(root);
        {
            let mut animator = lock(&services.companion);
            animator.select_owner(Some((character, root.to_bits())));
            for name in ["idle-one.bvh", "talking-one.bvh"] {
                animator.cache.push_back((name.into(), clip.clone()));
            }
        }
        let reaction = PathBuf::from("reaction.bvh");
        for speech_continues in [true, false] {
            app.world_mut().resource_mut::<Fixture>().playing = true;
            app.world_mut()
                .resource_mut::<AnimationState>()
                .select_one_shot(clip.clone(), Some(reaction.clone()));
            for _ in 0..3 {
                app.update();
            }
            assert_eq!(
                app.world().resource::<AnimationState>().path.as_ref(),
                Some(&reaction)
            );
            assert!(app.world().resource::<AnimationState>().gesture_active());
            app.world_mut().resource_mut::<Fixture>().playing = speech_continues;
            let destination = PathBuf::from(if speech_continues {
                "talking-one.bvh"
            } else {
                "idle-one.bvh"
            });
            for _ in 0..25 {
                app.update();
                if app.world().resource::<AnimationState>().path.as_ref() == Some(&destination) {
                    break;
                }
            }
            let animation = app.world().resource::<AnimationState>();
            assert_eq!(animation.path.as_ref(), Some(&destination));
            assert!(animation.playing && animation.looping && !animation.gesture_active());
            app.world_mut().resource_mut::<Fixture>().playing = false;
            app.update();
            assert_eq!(
                app.world().resource::<AnimationState>().path.as_deref(),
                Some(std::path::Path::new("idle-one.bvh"))
            );
        }
        drop(app);
        std::fs::remove_dir_all(root_dir).unwrap();
    }
    #[test]
    fn manual_motion_blocks_future_reply_gestures_and_resume_does_not_replay_them() {
        use waifu_studio::{
            data::{Character, EmotionMotionBinding, Store},
            reply_affect::AffectEmotion,
        };
        let root =
            std::env::temp_dir().join(format!("manual-reply-motion-{}", uuid::Uuid::new_v4()));
        let (store, mut library) = Store::open(root.clone()).unwrap();
        let character = uuid::Uuid::new_v4();
        library.characters.push(Character {
            delegation: Default::default(),
            id: character,
            name: "Fixture".into(),
            model: "unused.vrm".into(),
            persona: String::new(),
            format: "VRM".into(),
            greeting: String::new(),
            scene: None,
            provider: None,
            voice: None,
        });
        library.selected = Some(character);
        library.scene.reply_motion.mode = waifu_studio::data::ReplyMotionMode::EmotionMapped;
        library
            .scene
            .reply_motion
            .emotion_bindings
            .push(EmotionMotionBinding {
                emotion: AffectEmotion::Happy,
                path: "joy.bvh".into(),
            });
        let mut studio = Studio::new(store, library);
        let reply = uuid::Uuid::new_v4();
        studio.completed_chat = Some((reply, Ok("A completed reply".into())));
        studio.completed_affect = Some((reply, character, AffectEmotion::Happy));
        let entries = std::sync::Arc::new(vec![motion_library::MotionEntry {
            metadata: None,
            group: "fixture".into(),
            label: "Joy".into(),
            path: "joy.bvh".into(),
            format: "BVH",
        }]);
        let services = MotionServices::default();
        let ctx = egui::Context::default();
        bind_motion_services(&ctx, &services);
        ctx.data_mut(|d| d.insert_temp(egui::Id::new("motion_library_entries"), entries.clone()));
        lock(&companion_shared(&ctx)).select_owner(Some((character, 1)));
        let mut animation = AnimationState::default();
        manual_motion(&ctx, &mut animation);
        assert!(
            lock(&services.companion).paused,
            "UI controls must mutate the host resource"
        );
        animation.path = Some("manual.bvh".into());
        reply_motion_frame(&ctx, &mut studio, &mut animation, Some(1), false);
        assert!(
            !animation.loading,
            "A new reply must not replace manual playback"
        );
        assert_eq!(
            animation.path.as_deref(),
            Some(std::path::Path::new("manual.bvh"))
        );
        lock(&companion_shared(&ctx)).resume();
        animation.resume_automatic_control();
        reply_motion_frame(&ctx, &mut studio, &mut animation, Some(1), false);
        assert!(
            !animation.loading,
            "Resume must not replay a reply consumed during manual control"
        );

        // A subsequent reply can request a gesture again. Exercise the real
        // selector without launching a file worker or a provider request.
        let (suggester, auto) = motion_shared(&ctx);
        let next = motion_decision::CompletedReply {
            id: uuid::Uuid::new_v4(),
            character,
            character_name: "Fixture",
            avatar: Some(1),
            text: "Another reply",
            affect: Some(AffectEmotion::Happy),
        };
        let action = lock(&auto).frame(
            std::time::Instant::now(),
            &studio.library.scene.reply_motion,
            Some(character),
            Some(1),
            Some(next),
            &entries,
            &mut lock(&suggester),
            |_| panic!("Emotion mapping never needs a decision catalog"),
            |_, _| panic!("Emotion mapping never sends a provider request"),
        );
        assert!(
            matches!(action, Some(motion_decision::AutoAction::Play(path)) if path == std::path::Path::new("joy.bvh"))
        );
        drop(studio);
        std::fs::remove_dir_all(root).unwrap();
    }
    use waifu_studio::{data::Store, listen::ListeningState, memory::MemoryState};

    #[test]
    fn model_dropdown_hides_catalog_until_open_and_closes_after_selection() {
        let models = vec!["vendor/current".to_owned(), "vendor/other".to_owned()];
        let ctx = egui::Context::default();
        style(&ctx);
        let mut current = models[0].clone();
        let mut draw = |ui: &mut egui::Ui| {
            if let Some(model) = model_dropdown(ui, "test_selector", &models, &current) {
                current = model;
            }
        };
        render(&ctx, vec![], &mut draw);
        let closed = render(&ctx, vec![], &mut draw);
        assert!(!closed.shapes.iter().any(|s| matches!(&s.shape,
            egui::Shape::Text(t) if t.galley.job.text.contains("matching"))));
        let selector = text_rect(&closed, "vendor/current").center();
        render(&ctx, click(selector), &mut draw);
        let open = render(&ctx, vec![], &mut draw);
        text_rect(&open, "2 matching");
        let choice = text_rect(&open, "vendor/other").center();
        render(&ctx, click(choice), &mut draw);
        let closed = render(&ctx, vec![], &mut draw);
        assert_eq!(current, "vendor/other");
        assert!(!closed.shapes.iter().any(|s| matches!(&s.shape,
            egui::Shape::Text(t) if t.galley.job.text.contains("matching"))));
    }

    #[test]
    fn model_browser_selects_entries_beyond_the_old_two_hundred_limit() {
        let models: Vec<String> = (0..500)
            .map(|index| format!("vendor/model-{index:04}"))
            .collect();
        let ctx = egui::Context::default();
        style(&ctx);
        let mut selected = None;
        let mut frame = |search: &str, events| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        EVec2::new(800.0, 600.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    ui.set_width(520.0);
                    let id = ui.make_persistent_id(("catalog_test", "search"));
                    ui.ctx()
                        .data_mut(|data| data.insert_temp(id, search.to_owned()));
                    if let Some(model) = model_list(ui, "catalog_test", &models, "") {
                        selected = Some(model);
                    }
                },
            );
            output.textures_delta.clear();
            output
        };
        frame("", vec![]);
        let output = frame("", vec![]);
        text_rect(&output, "500 matching");
        frame("model-0499", vec![]);
        let output = frame("model-0499", vec![]);
        let pos = text_rect(&output, "vendor/model-0499").center();
        frame(
            "model-0499",
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
        assert_eq!(selected.as_deref(), Some("vendor/model-0499"));
    }

    fn text_rect(output: &egui::FullOutput, needle: &str) -> egui::Rect {
        fn locate(shape: &egui::Shape, needle: &str) -> Option<egui::Rect> {
            match shape {
                egui::Shape::Text(text) if text.galley.job.text.contains(needle) => {
                    Some(egui::Rect::from_min_size(text.pos, text.galley.size()))
                }
                egui::Shape::Vec(shapes) => shapes.iter().find_map(|s| locate(s, needle)),
                _ => None,
            }
        }
        output
            .shapes
            .iter()
            .find_map(|s| locate(&s.shape, needle))
            .unwrap_or_else(|| {
                panic!(
                    "Missing rendered text: {needle}; shapes: {:?}",
                    output
                        .shapes
                        .iter()
                        .filter_map(|s| if let egui::Shape::Text(t) = &s.shape {
                            Some((&t.galley.job.text, t.pos))
                        } else {
                            None
                        })
                        .collect::<Vec<_>>()
                )
            })
    }

    #[test]
    fn composer_picker_changes_real_model_and_preserves_draft() {
        let directory = std::env::temp_dir().join(format!("composer-ui-{}", uuid::Uuid::new_v4()));
        let (store, mut library) = Store::open(directory.clone()).unwrap();
        library.providers[0].model = "fixture-first-model".into();
        library.provider = 0;
        let mut studio = Studio::new(store, library);
        studio.composer = "Keep this multiline\ndraft intact".into();
        studio.model_lists.insert(
            0,
            vec!["fixture-first-model".into(), "fixture-second-model".into()],
        );
        let mut memory = MemoryState::load(directory.join("memory"));
        let mut semantic =
            waifu_studio::semantic_memory::SemanticMemory::load(directory.join("semantic"));
        let mut listening = ListeningState::default();
        let ctx = egui::Context::default();
        style(&ctx);
        let mut frame = |events| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        EVec2::new(1280.0, 900.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    composer(
                        ui.ctx(),
                        &mut studio,
                        &mut memory,
                        &mut semantic,
                        &mut listening,
                        70.0,
                        1280.0,
                    )
                },
            );
            output.textures_delta.clear();
            output
        };
        frame(vec![]);
        let output = frame(vec![]);
        let pos = text_rect(&output, "fixture-first-model").center();
        let click = |pos| {
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
        };
        frame(click(pos));
        let output = frame(vec![]);
        let pos = text_rect(&output, "fixture-second-model").center();
        frame(click(pos));
        let output = frame(vec![]);
        let pos = text_rect(&output, "Auto").center();
        frame(click(pos));
        let output = frame(vec![]);
        let pos = text_rect(&output, "High").center();
        frame(click(pos));
        frame(vec![]);
        assert_eq!(
            studio.library.providers[0].reasoning_effort,
            Some(waifu_studio::provider::ReasoningEffort::High)
        );
        assert_eq!(studio.library.providers[0].model, "fixture-second-model");
        assert_eq!(studio.composer, "Keep this multiline\ndraft intact");
        let (_, restored) = Store::open(directory.clone()).unwrap();
        assert_eq!(restored.providers[0].model, "fixture-second-model");
        assert_eq!(
            restored.providers[0].reasoning_effort,
            Some(waifu_studio::provider::ReasoningEffort::High)
        );
        drop(memory);
        let _ = std::fs::remove_dir_all(directory);
    }

    fn click(pos: egui::Pos2) -> Vec<egui::Event> {
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

    fn render(
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        mut add: impl FnMut(&mut egui::Ui),
    ) -> egui::FullOutput {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    EVec2::new(1400.0, 1000.0),
                )),
                events,
                ..Default::default()
            },
            |ui| add(ui),
        );
        output.textures_delta.clear();
        output
    }

    #[test]
    fn settings_page_paints_the_full_content_area_and_scrolls_at_viewport_height() {
        for size in [
            EVec2::new(2214., 1513.),
            EVec2::new(1280., 900.),
            EVec2::new(800., 600.),
        ] {
            let ctx = egui::Context::default();
            style(&ctx);
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
            let rail = if size.x >= 800. { 184. } else { 60. };
            let mut panel = egui::Rect::NOTHING;
            let mut viewport = egui::Rect::NOTHING;
            for _ in 0..3 {
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(screen),
                        ..default()
                    },
                    |ui| {
                        egui::Panel::left("fixture_navigation")
                            .exact_size(rail)
                            .resizable(false)
                            .show(ui, |_| {});
                        let response = settings_page(ui, |ui| {
                            kit::section(ui, "Voice & microphone");
                            viewport = egui::ScrollArea::vertical()
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    for index in 0..100 {
                                        ui.label(format!("Setting {index}"));
                                    }
                                })
                                .inner_rect;
                        });
                        panel = response.response.rect;
                    },
                );
                // Check actual painted coverage, not only a proposed rectangle.
                let opaque_background = output.shapes.iter().any(|shape| {
                    matches!(&shape.shape, egui::Shape::Rect(rect)
                        if rect.fill == SURFACE
                        && rect.rect.contains(egui::pos2(size.x - 2., size.y - 2.))
                        && rect.rect.contains(egui::pos2(rail + 3., 3.)))
                });
                output.textures_delta.clear();
                assert!(
                    opaque_background,
                    "Settings must hide the scene across the full page at {size:?}"
                );
            }
            assert!((panel.bottom() - screen.bottom()).abs() < 2., "{panel:?}");
            assert!((panel.right() - screen.right()).abs() < 2., "{panel:?}");
            assert!(viewport.height() > size.y - 150., "{viewport:?}");
            assert!(viewport.right() < screen.right());
        }
    }

    #[test]
    fn original_personality_never_silently_replaces_custom_text() {
        let riko = &personality_catalog::ORIGINALS[1];
        let neuro = &personality_catalog::ORIGINALS[0];
        let mut persona = String::new();
        assert!(apply_original(&mut persona, riko, false));
        assert_eq!(persona, riko.system_prompt);
        assert!(apply_original(&mut persona, neuro, false));
        assert_eq!(persona, neuro.system_prompt);
        let mut custom = "my own character".to_owned();
        assert!(!apply_original(&mut custom, riko, false));
        assert_eq!(custom, "my own character");
        assert!(apply_original(&mut custom, riko, true));
        assert_eq!(custom, riko.system_prompt);
    }

    fn character(name: &str, model: &str, persona: &str) -> waifu_studio::data::Character {
        waifu_studio::data::Character {
            delegation: Default::default(),
            id: uuid::Uuid::new_v4(),
            name: name.into(),
            model: model.into(),
            persona: persona.into(),
            format: "VRM 0.0".into(),
            greeting: String::new(),
            scene: None,
            provider: None,
            voice: None,
        }
    }

    #[test]
    fn companion_dropdown_keeps_other_profiles_out_of_the_editor_until_opened() {
        let directory =
            std::env::temp_dir().join(format!("companion-dropdown-{}", uuid::Uuid::new_v4()));
        let (store, mut library) = Store::open(directory.clone()).unwrap();
        let first = character("First companion", "/models/first.vrm", "first prompt");
        let second = character("Second companion", "/models/second.vrm", "second prompt");
        let second_id = second.id;
        library.selected = Some(first.id);
        library.characters = vec![first, second];
        let mut studio = Studio::new(store, library);
        let ctx = egui::Context::default();
        style(&ctx);
        render(&ctx, vec![], |ui| characters(ui, &mut studio));
        let closed = render(&ctx, vec![], |ui| characters(ui, &mut studio));
        let selector = text_rect(&closed, "First companion").center();
        render(&ctx, click(selector), |ui| characters(ui, &mut studio));
        let open = render(&ctx, vec![], |ui| characters(ui, &mut studio));
        let choice = text_rect(&open, "Second companion").center();
        render(&ctx, click(choice), |ui| characters(ui, &mut studio));
        assert_eq!(studio.library.selected, Some(second_id));
        render(&ctx, vec![], |ui| characters(ui, &mut studio));
        assert_eq!(
            studio.library.selected_character().unwrap().persona,
            "second prompt"
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn character_editor_applies_matching_original_and_confirms_custom_replacement() {
        let directory =
            std::env::temp_dir().join(format!("personality-ui-{}", uuid::Uuid::new_v4()));
        let (store, mut library) = Store::open(directory.clone()).unwrap();
        let riko = character("Riko", "/models/riko.vrm", "");
        let hikari = character("Hikari-chan", "/models/hikkyc2.vrm", "my own text");
        let (riko_id, hikari_id) = (riko.id, hikari.id);
        library.characters.extend([riko, hikari]);
        library.selected = Some(riko_id);
        let mut studio = Studio::new(store, library);
        let ctx = egui::Context::default();
        style(&ctx);

        render(&ctx, vec![], |ui| character_editor(ui, &mut studio));
        let persona = |studio: &Studio, id: uuid::Uuid| {
            studio
                .library
                .characters
                .iter()
                .find(|c| c.id == id)
                .unwrap()
                .persona
                .clone()
        };
        assert_eq!(
            persona(&studio, riko_id),
            personality_catalog::ORIGINALS[1].system_prompt
        );
        assert!(studio.notice.as_deref().unwrap().contains("Riko"));
        let (_, restored) = Store::open(directory.clone()).unwrap();
        assert_eq!(
            restored
                .characters
                .iter()
                .find(|c| c.id == riko_id)
                .unwrap()
                .persona,
            personality_catalog::ORIGINALS[1].system_prompt
        );

        studio.select_character(hikari_id);
        render(&ctx, vec![], |ui| character_editor(ui, &mut studio));
        let output = render(&ctx, vec![], |ui| character_editor(ui, &mut studio));
        assert_eq!(persona(&studio, hikari_id), "my own text");
        let pos = text_rect(&output, "Your own").center();
        render(&ctx, click(pos), |ui| character_editor(ui, &mut studio));
        let output = render(&ctx, vec![], |ui| character_editor(ui, &mut studio));
        let pos = text_rect(&output, "Neuro-sama").center();
        render(&ctx, click(pos), |ui| character_editor(ui, &mut studio));
        let output = render(&ctx, vec![], |ui| character_editor(ui, &mut studio));
        assert_eq!(persona(&studio, hikari_id), "my own text");
        text_rect(&output, "Replace your text with Neuro-sama?");
        let pos = text_rect(&output, "Replace").center();
        render(&ctx, click(pos), |ui| character_editor(ui, &mut studio));
        assert_eq!(
            persona(&studio, hikari_id),
            personality_catalog::ORIGINALS[0].system_prompt
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn motion_dropdown_opens_and_loads_the_selected_motion() {
        let entry = |group: &str, label: &str, format| motion_library::MotionEntry {
            metadata: None,
            group: group.into(),
            label: label.into(),
            path: std::path::PathBuf::from(format!("/lib/{group}/{label}")),
            format,
        };
        let entries = vec![
            entry("Mixamo", "Wave", "FBX"),
            entry("sachi-vrma", "rightwave1", "VRMA"),
            entry("silly-bvh", "joy3", "BVH"),
        ];
        assert_eq!(
            filter_motions(&entries, Some("silly-bvh"), "", None).len(),
            1
        );
        assert_eq!(filter_motions(&entries, None, "WAVE", None).len(), 2);
        assert_eq!(
            filter_motions(
                &entries,
                None,
                "",
                Some(motion_library::MotionCategory::Greeting)
            )
            .len(),
            2
        );
        let ctx = egui::Context::default();
        style(&ctx);
        let mut picked = None;
        render(&ctx, vec![], |ui| {
            motion_picker(ui, &entries, None);
        });
        let output = render(&ctx, vec![], |ui| {
            motion_picker(ui, &entries, None);
        });
        let selector = text_rect(&output, "Choose a motion…").center();
        render(&ctx, click(selector), |ui| {
            motion_picker(ui, &entries, None);
        });
        let output = render(&ctx, vec![], |ui| {
            motion_picker(ui, &entries, None);
        });
        text_rect(&output, "3 motions");
        let entry = text_rect(&output, "rightwave1").center();
        render(&ctx, click(entry), |ui| {
            if let Some(path) = motion_picker(ui, &entries, None) {
                picked = Some(path);
            }
        });
        assert_eq!(
            picked,
            Some(std::path::PathBuf::from("/lib/sachi-vrma/rightwave1"))
        );
    }

    #[test]
    fn suggest_motion_is_opt_in_and_plays_only_the_reviewed_scanned_entry() {
        let entry = |label: &str| motion_library::MotionEntry {
            metadata: None,
            group: "lib".into(),
            label: label.into(),
            path: std::path::PathBuf::from(format!("/lib/{label}.bvh")),
            format: "BVH",
        };
        let entries = vec![entry("Wave"), entry("Idle"), entry("walk")];
        let character = uuid::Uuid::new_v4();
        let mut suggester = motion_decision::MotionSuggester::default();
        let ctx = egui::Context::default();
        style(&ctx);
        let mut auto = motion_decision::ReplyMotion::default();
        let mut config = waifu_studio::data::ReplyMotionConfig {
            mode: waifu_studio::data::ReplyMotionMode::Off,
            ..Default::default()
        };
        let mut frame = |events, suggester: &mut motion_decision::MotionSuggester| {
            let mut action = None;
            let output = render(&ctx, events, |ui| {
                suggester.poll(Some(character), &entries);
                let (a, changed) = motion_suggestion(
                    ui,
                    suggester,
                    &mut auto,
                    &mut config,
                    &entries,
                    Some((character, "Hikari")),
                    "Hi everyone!",
                    None,
                    |_| {},
                );
                assert!(!changed, "automatic mode is never enabled silently");
                if a.is_some() {
                    action = a;
                }
            });
            (output, action)
        };
        frame(vec![], &mut suggester);
        let (output, action) = frame(vec![], &mut suggester);
        assert!(action.is_none(), "viewing the panel requests nothing");
        text_rect(&output, "Emotion-driven gestures");
        for _ in 0..30 {
            frame(vec![], &mut suggester);
        }
        let (output, _) = frame(vec![], &mut suggester);
        text_rect(&output, "Off");
        let button = text_rect(&output, "Suggest motion").center();
        let (_, action) = frame(click(button), &mut suggester);
        let Some(SuggestAction::Start(request)) = action else {
            panic!("Suggest motion should start a request");
        };
        assert_eq!(request.context, "Hi everyone!");
        let labels: Vec<_> = request.catalog.options().map(|o| o.label.clone()).collect();
        assert_eq!(labels, ["Wave", "Idle"], "locomotion is not offered");
        suggester
            .start_with(request, |request, _| {
                Ok(request
                    .catalog
                    .options()
                    .find(|o| o.label == "Wave")
                    .map(|o| o.id.clone()))
            })
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while suggester.busy() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
            suggester.poll(Some(character), &entries);
        }
        let (output, action) = frame(vec![], &mut suggester);
        assert!(action.is_none(), "a ready suggestion never plays by itself");
        text_rect(&output, "Suggested: Wave");
        let play = text_rect(&output, "Play suggestion").center();
        let (_, action) = frame(click(play), &mut suggester);
        let Some(SuggestAction::Play(path)) = action else {
            panic!("Play suggestion should load the motion");
        };
        assert_eq!(path, std::path::PathBuf::from("/lib/Wave.bvh"));
    }
    #[test]
    fn emotion_dropdown_keeps_multiple_choices_and_removes_unavailable_bindings() {
        let entry = |name: &str| motion_library::MotionEntry {
            metadata: None,
            group: "lib".into(),
            label: name.into(),
            path: format!("/lib/{name}.bvh").into(),
            format: "BVH",
        };
        let entries = vec![entry("Wave"), entry("Idle"), entry("walk")];
        let ctx = egui::Context::default();
        style(&ctx);
        let mut config = waifu_studio::data::ReplyMotionConfig {
            mode: waifu_studio::data::ReplyMotionMode::Off,
            ..Default::default()
        };
        let frame = |events, config: &mut waifu_studio::data::ReplyMotionConfig| {
            render(&ctx, events, |ui| {
                emotion_bindings(ui, config, &entries);
            })
        };
        frame(vec![], &mut config);
        let closed = frame(vec![], &mut config);
        frame(click(text_rect(&closed, "None").center()), &mut config);
        let open = frame(vec![], &mut config);
        frame(click(text_rect(&open, "lib / Wave").center()), &mut config);
        let open = frame(vec![], &mut config);
        // The popup stays open after a checkbox click, so choices accumulate.
        frame(click(text_rect(&open, "lib / Idle").center()), &mut config);
        let open = frame(vec![], &mut config);
        text_rect(&open, "lib / Wave");
        assert!(!open.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Text(text) if text.galley.job.text.contains("lib / walk"))));
        assert_eq!(
            config
                .emotion_bindings
                .iter()
                .map(|b| b.path.clone())
                .collect::<Vec<_>>(),
            [
                std::path::PathBuf::from("/lib/Wave.bvh"),
                std::path::PathBuf::from("/lib/Idle.bvh")
            ]
        );
        assert!(
            config
                .emotion_bindings
                .iter()
                .all(|b| b.emotion == waifu_studio::reply_affect::AffectEmotion::Neutral)
        );
        assert_eq!(
            config.mode,
            waifu_studio::data::ReplyMotionMode::Off,
            "editing bindings never enables automatic playback"
        );
        // Other closed emotion rows also say None. Target the foreground
        // popup item, which paints after the main form.
        let none = open
            .shapes
            .iter()
            .rev()
            .find_map(|shape| {
                if let egui::Shape::Text(text) = &shape.shape {
                    (text.galley.job.text == "None")
                        .then(|| egui::Rect::from_min_size(text.pos, text.galley.size()))
                } else {
                    None
                }
            })
            .unwrap();
        frame(click(none.center()), &mut config);
        assert!(config.emotion_bindings.is_empty());
        // Close the popup, then seed a missing binding and reopen it.
        frame(click(egui::pos2(1300.0, 900.0)), &mut config);
        config
            .emotion_bindings
            .push(waifu_studio::data::EmotionMotionBinding {
                emotion: waifu_studio::reply_affect::AffectEmotion::Neutral,
                path: "/gone/old.bvh".into(),
            });
        let closed = frame(vec![], &mut config);
        frame(
            click(text_rect(&closed, "Missing from scan").center()),
            &mut config,
        );
        let open = frame(vec![], &mut config);
        frame(
            click(text_rect(&open, "Unavailable: old.bvh").center()),
            &mut config,
        );
        assert!(config.emotion_bindings.is_empty());
    }
}
