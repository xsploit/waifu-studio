use bevy_egui::egui;
use waifu_studio::agents::{Access, AgentProvider, AgentState, Transport};
use waifu_studio::settings_kit as kit;
pub fn ui(ui: &mut egui::Ui, state: &mut AgentState) {
    kit::section(ui, "Coding agents");
    kit::note(
        ui,
        "Run an installed CLI on one explicitly reviewed task. No chat-triggered execution. Installed CLI authentication/environment is inherited; this app stores no credentials.",
    );
    kit::note(
        ui,
        "Read only is the default. File edits explicitly authorizes the preset's editing tools. Codex uses its CLI OS sandbox; Claude and Pi use tool restrictions, not an OS sandbox. No permission bypass flags are used. Custom commands use their supplied arguments with your user permissions. Process groups stop on cancel/timeout on Unix; independently detached descendants may escape.",
    );
    let busy = state.is_busy();
    ui.add_enabled_ui(!busy, |ui| {
        let mut changed = false;
        kit::row(ui, "Transport", "Where the installed CLI runs.", |ui| {
            changed |= ui.selectable_value(&mut state.transport, Transport::Ssh, "SSH host").changed();
            changed |= ui.selectable_value(&mut state.transport, Transport::Local, "Local").changed();
        });
        if state.transport == Transport::Ssh {
            kit::note(ui, "Remote installed CLI/authentication. SSH requires a known host key and noninteractive authentication. No keys are read by this app. GNU timeout must be installed remotely. Cancel closes SSH; detached processes and prior file changes may survive.");
            changed |= kit::text_row(ui, "Host", "", &mut state.ssh.host, "").changed();
            changed |= kit::text_row(ui, "User", "", &mut state.ssh.user, "").changed();
            changed |= kit::row(ui, "Port", "", |ui| ui.add(egui::DragValue::new(&mut state.ssh.port).range(1..=65535))).changed();
            changed |= kit::text_row(ui, "Identity file", "Optional path passed to ssh; never loaded by this app.", &mut state.ssh.identity_file, "").changed();
            changed |= kit::text_row(ui, "Remote directory", "Absolute POSIX working directory.", &mut state.ssh.working_dir, "").changed();
        }

        kit::row(ui, "Coding agent", "Installed provider CLI.", |ui| {
        egui::ComboBox::from_id_salt("coding_agent_provider")
            .selected_text(state.provider.label())
            .show_ui(ui, |ui| {
                for provider in [
                    AgentProvider::Codex,
                    AgentProvider::Claude,
                    AgentProvider::Pi,
                    AgentProvider::Custom,
                ] {
                    changed |= ui
                        .selectable_value(&mut state.provider, provider, provider.label())
                        .changed();
                }
            });
        });
        if state.provider != AgentProvider::Custom {
            kit::row(ui, "Task access", "Review permissions before running.", |ui| {
                changed |= ui.selectable_value(&mut state.access, Access::FileEdits, "Allow file edits").changed();
                changed |= ui.selectable_value(&mut state.access, Access::ReadOnly, "Read only").changed();
            });
            if state.access == Access::FileEdits {
                kit::note(ui, "This task may modify files. Review the directory, permissions and exact command before Start. Claude/Pi editing tools are not confined by an app OS sandbox; Codex workspace-write follows its CLI sandbox policy. No Bash tool is enabled for Claude/Pi.");
            }
        }
        if state.provider == AgentProvider::Custom {
            changed |= kit::text_row(ui, "Executable", "Path or installed program name; no shell.", &mut state.executable, "").changed();
            let id = ui.id().with("agent_argument_json");
            let mut arguments = ui
                .data_mut(|d| d.get_temp::<String>(id))
                .unwrap_or_else(|| serde_json::to_string(&state.args).unwrap_or_default());
            ui.label(
                "Arguments as a JSON string array. Your task is appended as one final argument.",
            );
            if ui.text_edit_multiline(&mut arguments).changed() {
                changed = true;
                match serde_json::from_str::<Vec<String>>(&arguments) {
                    Ok(args) => state.args = args,
                    Err(_) => {
                        state.args = vec!["\0".into()];
                        state.error = Some("Arguments must be a valid JSON string array".into());
                    }
                }
            }
            ui.data_mut(|d| d.insert_temp(id, arguments));
        }
        if state.transport == Transport::Local {
            changed |= kit::text_row(ui, "Working directory", "Existing local directory.", &mut state.working_dir, "").changed();
        }
        ui.label("Task (maximum 32 KiB; passed literally, not as shell code)");
        changed |= ui
            .add(egui::TextEdit::multiline(&mut state.prompt).desired_rows(4))
            .changed();
        changed |= kit::slider_row(ui, "Timeout", "Seconds before the process is stopped.", egui::Slider::new(&mut state.timeout_seconds, 1..=600))
            .changed();
        if changed {
            state.disarm();
        }
        if ui.button("Review exact task").clicked()
            && let Err(e) = state.review()
        {
            state.error = Some(e);
        }
        if let Some(command) = state.reviewed_command() {
            ui.add(egui::Label::new(egui::RichText::new(command).monospace()).wrap());
        }
        if ui
            .add_enabled(
                state.reviewed_command().is_some(),
                egui::Button::new("Start reviewed task"),
            )
            .clicked()
            && let Err(e) = state.run()
        {
            state.error = Some(e);
        }
    });
    if busy && ui.button("Cancel and stop process group").clicked() {
        state.cancel();
    }
    ui.label(&state.status);
    if let Some(error) = &state.error {
        ui.colored_label(egui::Color32::RED, error);
    }
    if !state.output.is_empty() {
        ui.label("Agent output (untrusted; not automatically sent to chat)");
        egui::ScrollArea::vertical()
            .id_salt("agent_output")
            .max_height(220.0)
            .show(ui, |ui| {
                ui.add(egui::Label::new(egui::RichText::new(&state.output).monospace()).wrap());
            });
    }
    if busy {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(100));
    }
}
