use bevy_egui::egui;
use waifu_studio::settings_kit as kit;
use waifu_studio::{
    agents::{AgentState, Transport},
    task_board::{MAX_OUTSTANDING, MAX_RETAINED, TaskBoard, TaskConfig, TaskStatus},
};

pub fn ui(ui: &mut egui::Ui, board: &mut TaskBoard, current: &AgentState) {
    kit::section(ui, "Task board");
    ui.small("Add tasks, review their commands, then run the batch.")
        .on_hover_text("Each card copies the current coding-agent provider, directory, transport, access, and command. Cards are immutable. Installed CLI authentication is inherited at execution. Hermes works through Custom executable and arguments.");
    ui.label(format!(
        "{} / {MAX_OUTSTANDING} outstanding · {} running · last {MAX_RETAINED} results retained",
        board.outstanding_count(),
        board.running_count()
    ));
    let mut concurrency = board.concurrency();
    if ui
        .add_enabled_ui(board.running_count() == 0, |ui| {
            kit::slider_row(
                ui,
                "Concurrent tasks",
                "Change while no tasks are running.",
                egui::Slider::new(&mut concurrency, 1..=4),
            )
        })
        .inner
        .changed()
        && let Err(error) = board.set_concurrency(concurrency)
    {
        board.error = Some(error);
    }
    ui.collapsing("Add task", |ui| {
        let scope = TaskConfig::snapshot(current);
        ui.label(format!(
            "{} · {:?} · {} · {}",
            scope.provider.label(),
            scope.transport,
            scope.access.label(),
            if scope.transport == Transport::Ssh {
                &scope.ssh.working_dir
            } else {
                &scope.working_dir
            }
        ));
        kit::row(ui, "Title", "Up to 120 characters.", |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut board.draft_title)
                    .char_limit(120)
                    .desired_width(240.0),
            );
        });
        ui.label("Task instructions");
        ui.add(
            egui::TextEdit::multiline(&mut board.draft_prompt)
                .hint_text("Independent task instructions")
                .desired_rows(3)
                .char_limit(32768),
        );
        if ui
            .add_enabled(
                board.outstanding_count() < MAX_OUTSTANDING,
                egui::Button::new("Add task card"),
            )
            .clicked()
        {
            match board.enqueue(board.draft_title.clone(), board.draft_prompt.clone(), scope) {
                Ok(_) => {
                    board.draft_title.clear();
                    board.draft_prompt.clear();
                }
                Err(error) => board.error = Some(error),
            }
        }
    });
    ui.horizontal_wrapped(|ui| {
        if ui.button("Review pending").clicked()
            && let Err(error) = board.review_pending()
        {
            board.error = Some(error);
        }
        if ui
            .add_enabled(
                board.reviewed_count() > 0,
                egui::Button::new(format!("Run reviewed ({})", board.reviewed_count())),
            )
            .clicked()
            && let Err(error) = board.run_reviewed()
        {
            board.error = Some(error);
        }
        if ui
            .add_enabled(
                board.outstanding_count() > 0,
                egui::Button::new("Cancel all"),
            )
            .clicked()
        {
            board.cancel_all();
        }
        if ui.button("Clear finished").clicked() {
            board.clear_finished();
        }
    });
    if board.reviewed_count() > 0 {
        ui.small("New cards need review. Use independent scopes for parallel tasks.")
            .on_hover_text("The reviewed commands below apply only to those cards. Parallel tasks sharing a directory may edit the same files.");
    }
    if let Some(error) = &board.error {
        ui.colored_label(egui::Color32::RED, error);
    }
    let mut cancel = None;
    let mut context = None;
    for card in board.cards() {
        ui.push_id(card.id(), |ui| {
            kit::card(ui, |card_ui| {
                let ui = &mut *card_ui.ui;
                ui.horizontal_wrapped(|ui| {
                    ui.strong(format!("#{} {}", card.id(), card.title()));
                    ui.label(card.status().label());
                    if !card.status().finished() && ui.small_button("Cancel card").clicked() {
                        cancel = Some(card.id());
                    }
                    if ui
                        .add_enabled(
                            card.can_add_context(),
                            egui::Button::new("Add result to composer"),
                        )
                        .clicked()
                    {
                        context = Some(card.id());
                    }
                });
                let scope = card.config();
                ui.label(format!(
                    "{} · {:?} · {}",
                    scope.provider.label(),
                    scope.transport,
                    scope.access.label()
                ));
                ui.label(card.detail());
                // Exact review remains visible before execution; users need not
                // expand hidden output to see which scope they authorize.
                if card.status() == TaskStatus::Reviewed
                    && let Some(command) = card.review()
                {
                    ui.add(egui::Label::new(egui::RichText::new(command).monospace()).wrap());
                }
                ui.collapsing("Advanced · task details", |ui| {
                    ui.add(
                        egui::Label::new(egui::RichText::new(format!("{scope:#?}")).monospace())
                            .wrap(),
                    );
                    ui.label(card.prompt());
                });
                if !card.output().is_empty() {
                    ui.collapsing("Output · untrusted", |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt("output")
                            .max_height(180.0)
                            .show(ui, |ui| {
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(card.output()).monospace(),
                                    )
                                    .wrap(),
                                );
                            });
                    });
                }
            });
        });
    }
    if let Some(id) = cancel {
        board.cancel(id);
    }
    if let Some(id) = context
        && let Err(error) = board.stage_context(id)
    {
        board.error = Some(error);
    }
    if let Some(bytes) = board.peek_context_len() {
        ui.label(format!("Result staged ({bytes} bytes). If the draft is full, shorten it and click Add result again; the staged result is retained."));
    }
    if board.running_count() != 0 {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(100));
    }
}
