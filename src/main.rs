mod app;
use crossbeam_channel::{bounded, select, Receiver, Sender};
use eframe::egui;
use log::{error, info};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::thread;
use std::time::Duration;
use app::suspects::{ExpectedKind, SuspectRule, check_suspects_for_message};
use app::state::{AppState, parse_hex_bytes, parse_index_range, format_bytes_for_view, find_message_label, WatchView, WatchTarget, WatchItem, LabelRule, LeftPanelTab};
use app::net::spawn_connection;
use app::framing::frame_messages;

struct ByteBusterApp {
    state: AppState,
    reader_join: Option<thread::JoinHandle<()>>,
    writer_join: Option<thread::JoinHandle<()>>,
    incoming_buffer: Vec<u8>,
}

impl Default for ByteBusterApp {
    fn default() -> Self {
        Self {
            state: AppState::default(),
            reader_join: None,
            writer_join: None,
            incoming_buffer: Vec::new(),
        }
    }
}

impl eframe::App for ByteBusterApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Pump incoming data
        if let Some(rx) = &self.state.rx_from_reader {
            loop {
                match rx.try_recv() {
                    Ok(chunk) => {
                        self.incoming_buffer.extend_from_slice(&chunk);
                        // framing
                        let start = parse_hex_bytes(&self.state.start_pattern).unwrap_or_default();
                        let end = parse_hex_bytes(&self.state.end_pattern).unwrap_or_default();
                        for msg in frame_messages(&mut self.incoming_buffer, &start, &end) {
                            self.state.received_messages.push(msg);
                            if self.state.received_messages.len() > self.state.max_messages {
                                let overflow = self.state.received_messages.len() - self.state.max_messages;
                                self.state.received_messages.drain(0..overflow);
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
        }

        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            // Apply a base theme and tint the panels if a critical is active
            let mut visuals = egui::Visuals::dark();
            if self.state.critical_active {
                visuals.panel_fill = egui::Color32::from_rgb(60, 20, 20);
            }
            ctx.set_visuals(visuals);
            ui.heading("ByteBuster");
            ui.horizontal(|ui| {
                ui.label("Address");
                ui.text_edit_singleline(&mut self.state.address_input);
                if !self.state.is_connected {
                    if ui.button("Connect").clicked() {
                        match std::panic::catch_unwind({
                            let addr = self.state.address_input.clone();
                            move || spawn_connection(addr)
                        }) {
                            Ok((tx, rx, rj, wj)) => {
                                self.state.tx_to_writer = Some(tx);
                                self.state.rx_from_reader = Some(rx);
                                self.reader_join = Some(rj);
                                self.writer_join = Some(wj);
                                self.state.is_connected = true;
                                info!("connected");
                            }
                            Err(_) => {
                                error!("connect panic");
                            }
                        }
                    }
                } else {
                    if ui.button("Disconnect").clicked() {
                        self.state.is_connected = false;
                        self.state.tx_to_writer = None;
                        self.state.rx_from_reader = None;
                        self.reader_join.take();
                        self.writer_join.take();
                    }
                }

                ui.separator();
                ui.label("Send");
                let reserve_for_button = 90.0; // approximate width for the button
                let available = ui.available_width();
                let input_width = (available - reserve_for_button).max(120.0);
                let row_h = ui.spacing().interact_size.y; // match button height
                ui.add_sized(
                    [input_width, row_h],
                    egui::TextEdit::singleline(&mut self.state.send_hex_input)
                        .hint_text("hex bytes (e.g. FE ED FA CE)"),
                );
                if ui.button("Send").clicked() {
                    if let Some(tx) = &self.state.tx_to_writer {
                        match parse_hex_bytes(&self.state.send_hex_input) {
                            Ok(bytes) => { let _ = tx.send(bytes); }
                            Err(e) => { error!("send parse error: {}", e); }
                        }
                    }
                }
            });
        });

        egui::SidePanel::left("left").show(ctx, |ui| {
            ui.collapsing("Framing", |ui| {
                ui.label("Start bytes (hex, space-separated)");
                ui.text_edit_singleline(&mut self.state.start_pattern);
                ui.label("End bytes (hex, space-separated)");
                ui.text_edit_singleline(&mut self.state.end_pattern);
                ui.horizontal(|ui| {
                    ui.label("Unit size");
                    ui.radio_value(&mut self.state.unit_size, 1, "1");
                    ui.radio_value(&mut self.state.unit_size, 2, "2");
                    ui.radio_value(&mut self.state.unit_size, 4, "4");
                });
            });

            ui.separator();

            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.state.left_panel_tab, LeftPanelTab::Watch, "Watch list");
                ui.selectable_value(&mut self.state.left_panel_tab, LeftPanelTab::Labels, "Message labels");
                ui.selectable_value(&mut self.state.left_panel_tab, LeftPanelTab::Suspects, "Expected data");
            });
            ui.separator();

            if self.state.left_panel_tab == LeftPanelTab::Watch {
                ui.collapsing("Watch list", |ui| {
                let mut to_start_edit: Option<usize> = None;
                let mut to_save: Option<(usize, String, usize, usize)> = None;
                let mut to_delete: Option<usize> = None;
                let mut cancel_edit: bool = false;

                // Add form (stacked vertically, full width)
                egui::Frame::group(ui.style())
                    .inner_margin(egui::Margin::symmetric(10.0, 8.0))
                    .show(ui, |ui| {
                        ui.vertical(|ui| {
                            let w = ui.available_width();
                            ui.heading("Add watch item");
                            ui.add_space(6.0);
                            ui.label("Name");
                            ui.add_sized([w, 0.0], egui::TextEdit::singleline(&mut self.state.new_watch_name));
                            ui.label("Index or range");
                            ui.add_sized([w, 0.0], egui::TextEdit::singleline(&mut self.state.new_watch_range).hint_text("e.g. 4 or 4-5"));
                            ui.label("View");
                            egui::ComboBox::from_id_source("add_watch_view")
                                .width(w)
                                .selected_text(self.state.new_watch_view.to_string())
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(&mut self.state.new_watch_view, WatchView::Hex, "Hex");
                                    ui.selectable_value(&mut self.state.new_watch_view, WatchView::Text, "Text");
                                    ui.selectable_value(&mut self.state.new_watch_view, WatchView::Binary, "Binary");
                                });
                            ui.label("Target");
                            egui::ComboBox::from_id_source("add_watch_target")
                                .width(w)
                                .selected_text(self.state.new_watch_target.to_string())
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(&mut self.state.new_watch_target, WatchTarget::All, "All messages");
                                    for rule in &self.state.label_rules {
                                        ui.selectable_value(&mut self.state.new_watch_target, WatchTarget::Label(rule.name.clone()), rule.name.clone());
                                    }
                                });
                            ui.add_space(8.0);
                            if ui.add_sized([w, 0.0], egui::Button::new("Add watch")).clicked() {
                                if let Some((start, end)) = parse_index_range(&self.state.new_watch_range) {
                                    let (start_index, end_index) = if start <= end { (start, end) } else { (end, start) };
                                    self.state.watch_items.push(WatchItem {
                                        name: self.state.new_watch_name.clone(),
                                        start_index,
                                        end_index,
                                        view: self.state.new_watch_view,
                                        target: self.state.new_watch_target.clone(),
                                    });
                                    self.state.new_watch_name.clear();
                                    self.state.new_watch_range.clear();
                                    self.state.new_watch_view = WatchView::Hex;
                                    self.state.new_watch_target = WatchTarget::All;
                                }
                            }
                        });
                    });

                ui.add_space(6.0);
                ui.separator();
                ui.label("Current watch items");
                ui.add_space(4.0);

                for (i, item) in self.state.watch_items.iter().enumerate() {
                    egui::Frame::group(ui.style())
                        .inner_margin(egui::Margin::symmetric(10.0, 8.0))
                        .outer_margin(egui::Margin::symmetric(0.0, 4.0))
                        .show(ui, |ui| {
                            let w = ui.available_width();
                            ui.set_width(w);
                            if self.state.edit_watch_idx == Some(i) {
                                ui.vertical(|ui| {
                                    let w = ui.available_width();
                                    ui.label("Name");
                                    ui.add_sized([w, 0.0], egui::TextEdit::singleline(&mut self.state.edit_watch_name).hint_text("name"));
                                    ui.label("Index or range");
                                    ui.add_sized([w, 0.0], egui::TextEdit::singleline(&mut self.state.edit_watch_range).hint_text("index or range"));
                                    ui.label("View");
                                    egui::ComboBox::from_id_source(format!("edit_watch_view_{}", i))
                                        .width(w)
                                        .selected_text(self.state.edit_watch_view.to_string())
                                        .show_ui(ui, |ui| {
                                            ui.selectable_value(&mut self.state.edit_watch_view, WatchView::Hex, "Hex");
                                            ui.selectable_value(&mut self.state.edit_watch_view, WatchView::Text, "Text");
                                            ui.selectable_value(&mut self.state.edit_watch_view, WatchView::Binary, "Binary");
                                        });
                                    ui.label("Target");
                                    egui::ComboBox::from_id_source(format!("edit_watch_target_{}", i))
                                        .width(w)
                                        .selected_text(self.state.edit_watch_target.to_string())
                                        .show_ui(ui, |ui| {
                                            ui.selectable_value(&mut self.state.edit_watch_target, WatchTarget::All, "All messages");
                                            for rule in &self.state.label_rules {
                                                ui.selectable_value(&mut self.state.edit_watch_target, WatchTarget::Label(rule.name.clone()), rule.name.clone());
                                            }
                                        });
                                    ui.add_space(10.0);
                                    let btn_w = ui.available_width();
                                    let save_clicked = ui
                                        .add_sized([btn_w, 0.0], egui::Button::new("Save"))
                                        .clicked();
                                    if save_clicked {
                                        if let Some((s, e)) = parse_index_range(&self.state.edit_watch_range) {
                                            let (start, end) = if s <= e { (s, e) } else { (e, s) };
                                            to_save = Some((i, self.state.edit_watch_name.clone(), start, end));
                                        }
                                    }
                                    ui.add_space(4.0);
                                    if ui.add(egui::Button::new("Cancel").frame(false)).clicked() {
                                        cancel_edit = true;
                                    }
                                });
                            } else {
                                ui.vertical(|ui| {
                                    ui.strong(&item.name);
                                    ui.add_space(4.0);
                                    ui.monospace(format!("[{}..{}]", item.start_index, item.end_index));
                                    ui.add_space(2.0);
                                    ui.label(format!("{} | {}", item.view, item.target));
                                    ui.add_space(8.0);
                                    ui.horizontal(|ui| {
                                        if ui.button("Edit").clicked() { to_start_edit = Some(i); }
                                        if ui.button("Delete").clicked() { to_delete = Some(i); }
                                    });
                                });
                            }
                        });
                }

                if let Some(i) = to_start_edit {
                    self.state.edit_watch_idx = Some(i);
                    if let Some(item) = self.state.watch_items.get(i) {
                        self.state.edit_watch_name = item.name.clone();
                        self.state.edit_watch_range = format!("{}-{}", item.start_index, item.end_index);
                        self.state.edit_watch_view = item.view;
                        self.state.edit_watch_target = item.target.clone();
                    }
                }
                if let Some((i, name, start, end)) = to_save {
                    if let Some(item) = self.state.watch_items.get_mut(i) {
                        item.name = name;
                        item.start_index = start;
                        item.end_index = end;
                        item.view = self.state.edit_watch_view;
                        item.target = self.state.edit_watch_target.clone();
                    }
                    self.state.edit_watch_idx = None;
                    self.state.edit_watch_name.clear();
                    self.state.edit_watch_range.clear();
                    self.state.edit_watch_view = WatchView::Hex;
                    self.state.edit_watch_target = WatchTarget::All;
                }
                if cancel_edit {
                    self.state.edit_watch_idx = None;
                    self.state.edit_watch_name.clear();
                    self.state.edit_watch_range.clear();
                    self.state.edit_watch_view = WatchView::Hex;
                    self.state.edit_watch_target = WatchTarget::All;
                }
                if let Some(i) = to_delete {
                    if i < self.state.watch_items.len() {
                        self.state.watch_items.remove(i);
                    }
                    // Reset edit state if needed
                    self.state.edit_watch_idx = None;
                    self.state.edit_watch_name.clear();
                    self.state.edit_watch_range.clear();
                    self.state.edit_watch_view = WatchView::Hex;
                    self.state.edit_watch_target = WatchTarget::All;
                }
                });
            } else if self.state.left_panel_tab == LeftPanelTab::Labels {
                ui.collapsing("Message labels", |ui| {
                    let mut to_start_edit: Option<usize> = None;
                    let mut to_save: Option<(usize, String, usize, usize, Vec<u8>)> = None;
                    let mut to_delete: Option<usize> = None;
                    let mut cancel_edit: bool = false;

                    // Add form first (full width)
                    egui::Frame::group(ui.style())
                        .inner_margin(egui::Margin::symmetric(10.0, 8.0))
                        .show(ui, |ui| {
                            ui.vertical(|ui| {
                                let w = ui.available_width();
                                ui.heading("Add label rule");
                                ui.add_space(6.0);
                                ui.label("Name");
                                ui.add_sized([w, 0.0], egui::TextEdit::singleline(&mut self.state.new_label_name).hint_text("name"));
                                ui.label("Index or range");
                                ui.add_sized([w, 0.0], egui::TextEdit::singleline(&mut self.state.new_label_range).hint_text("e.g. 3 or 3-4"));
                                ui.label("Value hex");
                                ui.add_sized([w, 0.0], egui::TextEdit::singleline(&mut self.state.new_label_value_hex).hint_text("e.g. 01 or AA BB"));
                                ui.add_space(8.0);
                                if ui.add_sized([w, 0.0], egui::Button::new("Add label")).clicked() {
                                    if let Some((start, end)) = parse_index_range(&self.state.new_label_range) {
                                        if let Ok(value) = parse_hex_bytes(&self.state.new_label_value_hex) {
                                            let (start_index, end_index) = if start <= end { (start, end) } else { (end, start) };
                                            self.state.label_rules.push(LabelRule { name: self.state.new_label_name.clone(), start_index, end_index, value });
                                            self.state.new_label_name.clear();
                                            self.state.new_label_range.clear();
                                            self.state.new_label_value_hex.clear();
                                        }
                                    }
                                }
                            });
                        });

                    ui.add_space(6.0);
                    ui.separator();
                    ui.label("Current label rules");
                    ui.add_space(4.0);

                    for (i, rule) in self.state.label_rules.iter().enumerate() {
                        egui::Frame::group(ui.style())
                            .inner_margin(egui::Margin::symmetric(10.0, 8.0))
                            .outer_margin(egui::Margin::symmetric(0.0, 4.0))
                            .show(ui, |ui| {
                                let w = ui.available_width();
                                ui.set_width(w);
                                if self.state.edit_label_idx == Some(i) {
                                    ui.vertical(|ui| {
                                        ui.label("Name");
                                        ui.add_sized([w, 0.0], egui::TextEdit::singleline(&mut self.state.edit_label_name).hint_text("name"));
                                        ui.label("Index or range");
                                        ui.add_sized([w, 0.0], egui::TextEdit::singleline(&mut self.state.edit_label_range).hint_text("index or range"));
                                        ui.label("Value hex");
                                        ui.add_sized([w, 0.0], egui::TextEdit::singleline(&mut self.state.edit_label_value_hex).hint_text("e.g. 01 or AA BB"));
                                        ui.add_space(10.0);
                                        let save_clicked = ui
                                            .add_sized([w, 0.0], egui::Button::new("Save"))
                                            .clicked();
                                        if save_clicked {
                                            if let Some((s, e)) = parse_index_range(&self.state.edit_label_range) {
                                                if let Ok(value) = parse_hex_bytes(&self.state.edit_label_value_hex) {
                                                    let (start, end) = if s <= e { (s, e) } else { (e, s) };
                                                    to_save = Some((i, self.state.edit_label_name.clone(), start, end, value));
                                                }
                                            }
                                        }
                                        ui.add_space(4.0);
                                        if ui.add(egui::Button::new("Cancel").frame(false)).clicked() {
                                            cancel_edit = true;
                                        }
                                    });
                                } else {
                                    ui.vertical(|ui| {
                                        ui.strong(&rule.name);
                                        ui.add_space(4.0);
                                        ui.monospace(format!("[{}..{}] == {}", rule.start_index, rule.end_index, hex::encode_upper(&rule.value)));
                                        ui.add_space(8.0);
                                        ui.horizontal(|ui| {
                                            if ui.button("Edit").clicked() { to_start_edit = Some(i); }
                                            if ui.button("Delete").clicked() { to_delete = Some(i); }
                                        });
                                    });
                                }
                            });
                    }

                    if let Some(i) = to_start_edit {
                        self.state.edit_label_idx = Some(i);
                        if let Some(rule) = self.state.label_rules.get(i) {
                            self.state.edit_label_name = rule.name.clone();
                            self.state.edit_label_range = format!("{}-{}", rule.start_index, rule.end_index);
                            self.state.edit_label_value_hex = hex::encode_upper(&rule.value);
                        }
                    }
                    if let Some((i, name, start, end, value)) = to_save {
                        if let Some(rule) = self.state.label_rules.get_mut(i) {
                            rule.name = name;
                            rule.start_index = start;
                            rule.end_index = end;
                            rule.value = value;
                        }
                        self.state.edit_label_idx = None;
                        self.state.edit_label_name.clear();
                        self.state.edit_label_range.clear();
                        self.state.edit_label_value_hex.clear();
                    }
                    if cancel_edit {
                        self.state.edit_label_idx = None;
                        self.state.edit_label_name.clear();
                        self.state.edit_label_range.clear();
                        self.state.edit_label_value_hex.clear();
                    }
                    if let Some(i) = to_delete {
                        if i < self.state.label_rules.len() {
                            self.state.label_rules.remove(i);
                        }
                        self.state.edit_label_idx = None;
                        self.state.edit_label_name.clear();
                        self.state.edit_label_range.clear();
                        self.state.edit_label_value_hex.clear();
                    }
                });
            } else {
                ui.collapsing("Expected data", |ui| {
                    let mut to_start_edit: Option<usize> = None;
                    let mut to_save: Option<(usize, String, usize, usize, ExpectedKind, String, WatchTarget, app::suspects::Severity)> = None;
                    let mut to_delete: Option<usize> = None;
                    let mut cancel_edit: bool = false;

                    // Add form
                    egui::Frame::group(ui.style())
                        .inner_margin(egui::Margin::symmetric(10.0, 8.0))
                        .show(ui, |ui| {
                            ui.vertical(|ui| {
                                let w = ui.available_width();
                                ui.heading("Add expectation");
                                ui.add_space(6.0);
                                ui.label("Name");
                                ui.add_sized([w, 0.0], egui::TextEdit::singleline(&mut self.state.new_suspect_name));
                                ui.label("Index or range");
                                ui.add_sized([w, 0.0], egui::TextEdit::singleline(&mut self.state.new_suspect_range).hint_text("e.g. 10-13"));
                                ui.label("Expected kind");
                                egui::ComboBox::from_id_source("suspect_kind_add").width(w)
                                    .selected_text(match self.state.new_suspect_kind { app::suspects::ExpectedKind::Text => "Text", app::suspects::ExpectedKind::Hex => "Hex" })
                                    .show_ui(ui, |ui| {
                                        ui.selectable_value(&mut self.state.new_suspect_kind, app::suspects::ExpectedKind::Text, "Text");
                                        ui.selectable_value(&mut self.state.new_suspect_kind, app::suspects::ExpectedKind::Hex, "Hex");
                                    });
                                ui.label("Severity");
                                egui::ComboBox::from_id_source("suspect_severity_add").width(w)
                                    .selected_text(match self.state.new_suspect_severity { app::suspects::Severity::Info => "Info", app::suspects::Severity::Warning => "Warning", app::suspects::Severity::Critical => "Critical" })
                                    .show_ui(ui, |ui| {
                                        ui.selectable_value(&mut self.state.new_suspect_severity, app::suspects::Severity::Info, "Info");
                                        ui.selectable_value(&mut self.state.new_suspect_severity, app::suspects::Severity::Warning, "Warning");
                                        ui.selectable_value(&mut self.state.new_suspect_severity, app::suspects::Severity::Critical, "Critical");
                                    });
                                ui.label("Expected value");
                                let hint = match self.state.new_suspect_kind { app::suspects::ExpectedKind::Text => "e.g. PING", app::suspects::ExpectedKind::Hex => "e.g. 50 49 4E 47" };
                                ui.add_sized([w, 0.0], egui::TextEdit::singleline(&mut self.state.new_suspect_value).hint_text(hint));
                                ui.label("Target");
                                egui::ComboBox::from_id_source("suspect_target_add").width(w)
                                    .selected_text(self.state.new_suspect_target.to_string())
                                    .show_ui(ui, |ui| {
                                        ui.selectable_value(&mut self.state.new_suspect_target, WatchTarget::All, "All messages");
                                        for rule in &self.state.label_rules {
                                            ui.selectable_value(&mut self.state.new_suspect_target, WatchTarget::Label(rule.name.clone()), rule.name.clone());
                                        }
                                    });
                                ui.add_space(8.0);
                                if ui.add_sized([w, 0.0], egui::Button::new("Add expectation")).clicked() {
                                    if let Some((s, e)) = parse_index_range(&self.state.new_suspect_range) {
                                        let (start, end) = if s <= e { (s, e) } else { (e, s) };
                                        self.state.suspect_rules.push(SuspectRule {
                                            name: self.state.new_suspect_name.clone(),
                                            start_index: start,
                                            end_index: end,
                                            expected_kind: self.state.new_suspect_kind,
                                            expected_value: self.state.new_suspect_value.clone(),
                                            target: self.state.new_suspect_target.clone(),
                                            severity: self.state.new_suspect_severity,
                                        });
                                        self.state.new_suspect_name.clear();
                                        self.state.new_suspect_range.clear();
                                        self.state.new_suspect_value.clear();
                                        self.state.new_suspect_kind = app::suspects::ExpectedKind::Text;
                                        self.state.new_suspect_target = WatchTarget::All;
                                        self.state.new_suspect_severity = app::suspects::Severity::Warning;
                                    }
                                }
                            });
                        });

                    ui.add_space(6.0);
                    ui.separator();
                    ui.label("Current expectations");
                    ui.add_space(4.0);

                    for (i, r) in self.state.suspect_rules.iter().enumerate() {
                        egui::Frame::group(ui.style())
                            .inner_margin(egui::Margin::symmetric(10.0, 8.0))
                            .outer_margin(egui::Margin::symmetric(0.0, 4.0))
                            .show(ui, |ui| {
                                let w = ui.available_width();
                                ui.set_width(w);
                                if self.state.edit_suspect_idx == Some(i) {
                                    ui.vertical(|ui| {
                                        ui.label("Name");
                                        ui.add_sized([w, 0.0], egui::TextEdit::singleline(&mut self.state.edit_suspect_name));
                                        ui.label("Index or range");
                                        ui.add_sized([w, 0.0], egui::TextEdit::singleline(&mut self.state.edit_suspect_range));
                                        ui.label("Expected kind");
                                        egui::ComboBox::from_id_source(format!("suspect_kind_edit_{}", i))
                                            .width(w)
                                            .selected_text(match self.state.edit_suspect_kind { app::suspects::ExpectedKind::Text => "Text", app::suspects::ExpectedKind::Hex => "Hex" })
                                            .show_ui(ui, |ui| {
                                                ui.selectable_value(&mut self.state.edit_suspect_kind, app::suspects::ExpectedKind::Text, "Text");
                                                ui.selectable_value(&mut self.state.edit_suspect_kind, app::suspects::ExpectedKind::Hex, "Hex");
                                            });
                                        ui.label("Expected value");
                                        ui.add_sized([w, 0.0], egui::TextEdit::singleline(&mut self.state.edit_suspect_value));
                                        ui.label("Target");
                                        egui::ComboBox::from_id_source(format!("suspect_target_edit_{}", i))
                                            .width(w)
                                            .selected_text(self.state.edit_suspect_target.to_string())
                                            .show_ui(ui, |ui| {
                                                ui.selectable_value(&mut self.state.edit_suspect_target, WatchTarget::All, "All messages");
                                                for rule in &self.state.label_rules {
                                                    ui.selectable_value(&mut self.state.edit_suspect_target, WatchTarget::Label(rule.name.clone()), rule.name.clone());
                                                }
                                            });
                                        ui.label("Severity");
                                        egui::ComboBox::from_id_source(format!("suspect_severity_edit_{}", i))
                                            .width(w)
                                            .selected_text(match self.state.edit_suspect_severity { app::suspects::Severity::Info => "Info", app::suspects::Severity::Warning => "Warning", app::suspects::Severity::Critical => "Critical" })
                                            .show_ui(ui, |ui| {
                                                ui.selectable_value(&mut self.state.edit_suspect_severity, app::suspects::Severity::Info, "Info");
                                                ui.selectable_value(&mut self.state.edit_suspect_severity, app::suspects::Severity::Warning, "Warning");
                                                ui.selectable_value(&mut self.state.edit_suspect_severity, app::suspects::Severity::Critical, "Critical");
                                            });
                                        ui.add_space(10.0);
                                        let save_clicked = ui.add_sized([w, 0.0], egui::Button::new("Save")).clicked();
                                        if save_clicked {
                                            if let Some((s, e)) = parse_index_range(&self.state.edit_suspect_range) {
                                                let (start, end) = if s <= e { (s, e) } else { (e, s) };
                                                to_save = Some((
                                                    i,
                                                    self.state.edit_suspect_name.clone(),
                                                    start,
                                                    end,
                                                    self.state.edit_suspect_kind,
                                                    self.state.edit_suspect_value.clone(),
                                                    self.state.edit_suspect_target.clone(),
                                                    self.state.edit_suspect_severity,
                                                ));
                                            }
                                        }
                                        ui.add_space(4.0);
                                        if ui.add(egui::Button::new("Cancel").frame(false)).clicked() { cancel_edit = true; }
                                    });
                                } else {
                                    ui.vertical(|ui| {
                                        ui.strong(&r.name);
                                        ui.add_space(4.0);
                                        let kind = match r.expected_kind { app::suspects::ExpectedKind::Text => "Text", app::suspects::ExpectedKind::Hex => "Hex" };
                                        ui.monospace(format!("[{}..{}] {} -> {} ({})", r.start_index, r.end_index, kind, r.expected_value, match r.severity { app::suspects::Severity::Info => "Info", app::suspects::Severity::Warning => "Warning", app::suspects::Severity::Critical => "Critical" }));
                                        ui.add_space(8.0);
                                        ui.horizontal(|ui| {
                                            if ui.button("Edit").clicked() { to_start_edit = Some(i); }
                                            if ui.button("Delete").clicked() { to_delete = Some(i); }
                                        });
                                    });
                                }
                            });
                    }

                    if let Some(i) = to_start_edit {
                        self.state.edit_suspect_idx = Some(i);
                        if let Some(r) = self.state.suspect_rules.get(i) {
                            self.state.edit_suspect_name = r.name.clone();
                            self.state.edit_suspect_range = format!("{}-{}", r.start_index, r.end_index);
                            self.state.edit_suspect_kind = r.expected_kind;
                            self.state.edit_suspect_value = r.expected_value.clone();
                            self.state.edit_suspect_target = r.target.clone();
                            self.state.edit_suspect_severity = r.severity;
                        }
                    }
                    if let Some((i, name, start, end, kind, value, target, severity)) = to_save {
                        if let Some(r) = self.state.suspect_rules.get_mut(i) {
                            r.name = name;
                            r.start_index = start;
                            r.end_index = end;
                            r.expected_kind = kind;
                            r.expected_value = value;
                            r.target = target;
                            r.severity = severity;
                        }
                        self.state.edit_suspect_idx = None;
                        self.state.edit_suspect_name.clear();
                        self.state.edit_suspect_range.clear();
                        self.state.edit_suspect_value.clear();
                    }
                    if cancel_edit {
                        self.state.edit_suspect_idx = None;
                        self.state.edit_suspect_name.clear();
                        self.state.edit_suspect_range.clear();
                        self.state.edit_suspect_value.clear();
                    }
                    if let Some(i) = to_delete {
                        if i < self.state.suspect_rules.len() {
                            self.state.suspect_rules.remove(i);
                        }
                        self.state.edit_suspect_idx = None;
                        self.state.edit_suspect_name.clear();
                        self.state.edit_suspect_range.clear();
                        self.state.edit_suspect_value.clear();
                    }
                });
            }
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Incoming messages");
                if ui.button("Clear").clicked() {
                    self.state.received_messages.clear();
                    self.incoming_buffer.clear();
                    self.state.critical_active = false;
                }
                ui.add_space(8.0);
                ui.checkbox(&mut self.state.display_as_text, "Display as text");
            });
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                let mut any_critical = false;
                for (i, msg) in self.state.received_messages.iter().enumerate() {
                    ui.add_space(4.0);
                    egui::Frame::group(ui.style())
                        .outer_margin(egui::Margin::symmetric(0.0, 4.0))
                        .inner_margin(egui::Margin::symmetric(12.0, 10.0))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                let maybe_label = find_message_label(msg, &self.state.label_rules);
                                ui.strong(match maybe_label {
                                    Some(name) => name,
                                    None => format!("Message {}", i + 1),
                                });
                                ui.add_space(8.0);
                                ui.label(format!("{} bytes", msg.len()));
                            });
                            ui.add_space(6.0);
                            if self.state.display_as_text {
                                let text = String::from_utf8_lossy(msg);
                                ui.monospace(text);
                            } else {
                                ui.monospace(hex::encode_upper(msg));
                            }
                            // Suspected data warnings
                    let active_label = find_message_label(msg, &self.state.label_rules);
                            let warnings = check_suspects_for_message(msg, &active_label, &self.state.suspect_rules);
                    let mut critical = false;
                    for (sev, w) in warnings {
                        let _ = match sev {
                            app::suspects::Severity::Info => ui.label(format!("Note: {}", w)),
                            app::suspects::Severity::Warning => ui.colored_label(egui::Color32::YELLOW, format!("Warning: {}", w)),
                            app::suspects::Severity::Critical => { critical = true; ui.colored_label(egui::Color32::RED, format!("CRITICAL: {}", w)) }
                        };
                    }
                    any_critical = any_critical || critical;
                            if !self.state.watch_items.is_empty() {
                                ui.add_space(8.0);
                                ui.separator();
                                ui.add_space(6.0);
                                    egui::Grid::new(format!("watch_grid_{}", i))
                                        .striped(true)
                                        .num_columns(3)
                                        .show(ui, |ui| {
                                        let active_label = find_message_label(msg, &self.state.label_rules);
                                        for w in &self.state.watch_items {
                                            let target_applies = match (&w.target, &active_label) {
                                                (WatchTarget::All, _) => true,
                                                (WatchTarget::Label(name), Some(lbl)) => name == lbl,
                                                (WatchTarget::Label(_), None) => false,
                                            };
                                            if !target_applies { continue; }
                                            let start = w.start_index;
                                            let end = w.end_index;
                                            let slice = if start <= end && end < msg.len() { Some(&msg[start..=end]) } else { None };
                                            let value_str = match slice {
                                                Some(bytes) => match w.view {
                                                    WatchView::Hex => format!("0x{}", hex::encode_upper(bytes)),
                                                    WatchView::Text => format_bytes_for_view(bytes, WatchView::Text),
                                                    WatchView::Binary => format_bytes_for_view(bytes, WatchView::Binary),
                                                },
                                                None => "-".to_string(),
                                            };
                                            ui.label(&w.name);
                                            ui.monospace(format!("[{}..{}] {}", start, end, w.view));
                                            ui.monospace(value_str);
                                            ui.end_row();
                                        }
                                    });
                            }
                        });
                }
                // Update global critical state based on this frame's evaluation across all messages
                self.state.critical_active = any_critical;
            });
        });

        // Removed bottom send bar; sending controls are now in the top toolbar
    }
}

fn main() -> eframe::Result<()> {
    env_logger::init();
    let options = eframe::NativeOptions::default();
    eframe::run_native(
        "ByteBuster",
        options,
        Box::new(|_cc| Box::new(ByteBusterApp::default())),
    )
}
