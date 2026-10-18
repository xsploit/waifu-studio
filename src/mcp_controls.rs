//! MCP tool panel. Server metadata and results are shown as untrusted data;
//! a call runs only after Review → Execute on the exact tool and arguments.
use bevy_egui::egui;
use waifu_studio::mcp::McpState;

pub fn ui(ui: &mut egui::Ui, state: &mut McpState) {
    ui.heading("MCP tools");
    ui.label("Streamable HTTP only; no local commands are launched.");
    let busy = state.is_busy();
    ui.add_enabled_ui(!busy, |ui| {
        ui.horizontal(|ui| {
            ui.label("Endpoint");
            if ui.text_edit_singleline(&mut state.endpoint).changed() {
                state.disarm();
            }
        });
        ui.horizontal(|ui| {
            ui.label("Session token");
            ui.add(egui::TextEdit::singleline(&mut state.session_token).password(true))
                .on_hover_text("Optional bearer token, kept in memory for this session only");
        });
    });
    ui.horizontal(|ui| {
        if ui
            .add_enabled(!busy, egui::Button::new("List tools"))
            .clicked()
        {
            state.list_tools();
        }
        if busy && ui.button("Cancel").clicked() {
            state.cancel();
        }
    });
    ui.label(&state.status);
    if state.tools.is_empty() {
        return;
    }
    ui.separator();
    let mut picked = None;
    egui::ScrollArea::vertical()
        .id_salt("mcp_tools")
        .max_height(140.0)
        .show(ui, |ui| {
            for (i, tool) in state.tools.iter().enumerate() {
                let label = match &tool.title {
                    Some(title) => format!("{} ({title})", tool.name),
                    None => tool.name.clone(),
                };
                if ui
                    .selectable_label(state.selected == Some(i), label)
                    .clicked()
                {
                    picked = Some(i);
                }
            }
        });
    if let Some(i) = picked {
        state.select(i);
    }
    let Some(tool) = state.selected_tool().cloned() else {
        return;
    };
    ui.label(egui::RichText::new("Server-provided description (untrusted):").weak());
    ui.label(if tool.description.is_empty() {
        "(none)"
    } else {
        &tool.description
    });
    egui::CollapsingHeader::new("Input schema")
        .id_salt("mcp_schema")
        .show(ui, |ui| {
            let mut schema = serde_json::to_string_pretty(&tool.input_schema).unwrap_or_default();
            ui.add(
                egui::TextEdit::multiline(&mut schema)
                    .code_editor()
                    .interactive(false),
            );
        });
    ui.label("Arguments (JSON object)");
    let edited = ui
        .add_enabled(
            !busy,
            egui::TextEdit::multiline(&mut state.arguments)
                .code_editor()
                .desired_rows(4),
        )
        .changed();
    if edited {
        state.disarm();
    }
    match state.armed().map(|(t, a)| (t.to_owned(), a.to_owned())) {
        None => {
            if ui
                .add_enabled(
                    !busy,
                    egui::Button::new(format!("Review call to `{}`", tool.name)),
                )
                .clicked()
            {
                state.review();
            }
        }
        Some((name, args)) => {
            ui.label(format!(
                "About to call `{name}` on {} with:",
                state.endpoint.trim()
            ));
            ui.monospace(args);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(!busy, egui::Button::new(format!("Execute `{name}`")))
                    .clicked()
                {
                    state.execute();
                }
                if ui.button("Discard").clicked() {
                    state.disarm();
                }
            });
        }
    }
    if state.result.is_empty() {
        return;
    }
    ui.separator();
    ui.label(egui::RichText::new("Tool result (untrusted):").weak());
    egui::ScrollArea::vertical()
        .id_salt("mcp_result")
        .max_height(200.0)
        .show(ui, |ui| {
            let mut result = state.result.as_str();
            ui.add(egui::TextEdit::multiline(&mut result).code_editor());
        });
    if ui
        .button("Add result to composer")
        .on_hover_text("Appended as a labelled untrusted observation; review before sending")
        .clicked()
    {
        state.queue_context();
    }
}
