//! UI composition for ByteBuster.
use eframe::egui;
use log::{error, info};

use super::framing::frame_messages;
use super::net::spawn_connection;
use super::state::*;

/// Root eframe App implementation.
pub struct ByteBusterApp {
    pub state: AppState,
    pub reader_join: Option<std::thread::JoinHandle<()>>,
    pub writer_join: Option<std::thread::JoinHandle<()>>,
    pub incoming_buffer: Vec<u8>,
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
        // Pull incoming data and frame
        if let Some(rx) = &self.state.rx_from_reader {
            loop {
                match rx.try_recv() {
                    Ok(chunk) => {
                        self.incoming_buffer.extend_from_slice(&chunk);
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
                            Err(_) => { error!("connect panic"); }
                        }
                    }
                } else if ui.button("Disconnect").clicked() {
                    self.state.is_connected = false;
                    self.state.tx_to_writer = None;
                    self.state.rx_from_reader = None;
                    self.reader_join.take();
                    self.writer_join.take();
                }
                ui.checkbox(&mut self.state.display_as_text, "Display as text");
            });
        });

        super::ui_left_panel::render_left_panel(ctx, &mut self.state);
        super::ui_messages::render_messages(ctx, &mut self.state, &self.state.label_rules);

        egui::TopBottomPanel::bottom("bottom").show(ctx, |ui| {
            ui.horizontal(|ui| {
                let reserve_for_button = 100.0;
                let available = ui.available_width();
                let input_width = if available > reserve_for_button { available - reserve_for_button } else { (available * 0.7).max(0.0) };
                ui.add_sized([input_width, 0.0], egui::TextEdit::singleline(&mut self.state.send_hex_input).hint_text("Send hex bytes (e.g. FE ED FA CE)"));
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
    }
}

mod ui_left_panel {
    use super::*;
    pub fn render_left_panel(ctx: &egui::Context, state: &mut AppState) {
        egui::SidePanel::left("left").show(ctx, |ui| {
            ui.collapsing("Framing", |ui| {
                ui.label("Start bytes (hex, space-separated)");
                ui.text_edit_singleline(&mut state.start_pattern);
                ui.label("End bytes (hex, space-separated)");
                ui.text_edit_singleline(&mut state.end_pattern);
                ui.horizontal(|ui| {
                    ui.label("Unit size");
                    ui.radio_value(&mut state.unit_size, 1, "1");
                    ui.radio_value(&mut state.unit_size, 2, "2");
                    ui.radio_value(&mut state.unit_size, 4, "4");
                });
            });
            ui.separator();
            ui.horizontal(|ui| {
                ui.selectable_value(&mut state.left_panel_tab, LeftPanelTab::Watch, "Watch list");
                ui.selectable_value(&mut state.left_panel_tab, LeftPanelTab::Labels, "Message labels");
            });
            ui.separator();
            match state.left_panel_tab {
                LeftPanelTab::Watch => super::ui_watch::render_watch_list(ui, state),
                LeftPanelTab::Labels => super::ui_labels::render_labels(ui, state),
            }
        });
    }
}

mod ui_watch {
    use super::*;
    pub fn render_watch_list(ui: &mut egui::Ui, state: &mut AppState) {
        let _ = state;
        let _ = ui;
    }
}

mod ui_messages {
    use super::*;
    pub fn render_messages(ctx: &egui::Context, state: &mut AppState, rules: &[LabelRule]) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Incoming messages");
                if ui.button("Clear").clicked() {
                    state.received_messages.clear();
                }
            });
        });
    }
}

