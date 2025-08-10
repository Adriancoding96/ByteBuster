use crossbeam_channel::{bounded, select, Receiver, Sender};
use eframe::egui;
use log::{error, info};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::thread;
use std::time::Duration;
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WatchView {
    Hex,
    Text,
    Binary,
}

impl fmt::Display for WatchView {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WatchView::Hex => write!(f, "Hex"),
            WatchView::Text => write!(f, "Text"),
            WatchView::Binary => write!(f, "Binary"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum WatchTarget {
    All,
    Label(String),
}

impl fmt::Display for WatchTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WatchTarget::All => write!(f, "All messages"),
            WatchTarget::Label(name) => write!(f, "{}", name),
        }
    }
}

#[derive(Clone, Debug)]
struct WatchItem {
    name: String,
    start_index: usize,
    end_index: usize,
    view: WatchView,
    target: WatchTarget,
}

#[derive(Clone, Debug)]
struct LabelRule {
    name: String,
    start_index: usize,
    end_index: usize,
    value: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LeftPanelTab {
    Watch,
    Labels,
}

struct AppState {
    address_input: String,
    is_connected: bool,
    tx_to_writer: Option<Sender<Vec<u8>>>,
    rx_from_reader: Option<Receiver<Vec<u8>>>,
    received_messages: Vec<Vec<u8>>,
    max_messages: usize,
    display_as_text: bool,
    start_pattern: String,
    end_pattern: String,
    unit_size: usize,
    send_hex_input: String,
    watch_items: Vec<WatchItem>,
    new_watch_name: String,
    new_watch_range: String,
    edit_watch_idx: Option<usize>,
    edit_watch_name: String,
    edit_watch_range: String,
    new_watch_view: WatchView,
    edit_watch_view: WatchView,
    new_watch_target: WatchTarget,
    edit_watch_target: WatchTarget,
    label_rules: Vec<LabelRule>,
    new_label_name: String,
    new_label_range: String,
    new_label_value_hex: String,
    edit_label_idx: Option<usize>,
    edit_label_name: String,
    edit_label_range: String,
    edit_label_value_hex: String,
    left_panel_tab: LeftPanelTab,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            address_input: "127.0.0.1:9000".to_string(),
            is_connected: false,
            tx_to_writer: None,
            rx_from_reader: None,
            received_messages: Vec::new(),
            max_messages: 200,
            display_as_text: false,
            start_pattern: "AA 55".to_string(),
            end_pattern: "0D 0A".to_string(),
            unit_size: 1,
            send_hex_input: String::new(),
            watch_items: Vec::new(),
            new_watch_name: String::new(),
            new_watch_range: String::new(),
            edit_watch_idx: None,
            edit_watch_name: String::new(),
            edit_watch_range: String::new(),
            new_watch_view: WatchView::Hex,
            edit_watch_view: WatchView::Hex,
            new_watch_target: WatchTarget::All,
            edit_watch_target: WatchTarget::All,

            label_rules: Vec::new(),
            new_label_name: String::new(),
            new_label_range: String::new(),
            new_label_value_hex: String::new(),
            edit_label_idx: None,
            edit_label_name: String::new(),
            edit_label_range: String::new(),
            edit_label_value_hex: String::new(),

            left_panel_tab: LeftPanelTab::Watch,
        }
    }
}

fn parse_hex_bytes(input: &str) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    for token in input.split_whitespace() {
        let cleaned = token.trim_start_matches("0x").trim_start_matches("0X");
        if cleaned.is_empty() {
            continue;
        }
        let b = u8::from_str_radix(cleaned, 16).map_err(|e| format!("invalid hex '{}': {}", token, e))?;
        bytes.push(b);
    }
    Ok(bytes)
}

fn parse_index_range(input: &str) -> Option<(usize, usize)> {
    let s = input.trim();
    if s.is_empty() { return None; }
    if let Some((a, b)) = s.split_once('-') {
        let start = a.trim().parse::<usize>().ok()?;
        let end = b.trim().parse::<usize>().ok()?;
        Some((start, end))
    } else if let Ok(idx) = s.parse::<usize>() {
        Some((idx, idx))
    } else if let Some((a, b)) = s.split_once("..") {
        let start = a.trim().parse::<usize>().ok()?;
        let end = b.trim().parse::<usize>().ok()?;
        Some((start, end))
    } else {
        None
    }
}

fn format_bytes_for_view(bytes: &[u8], view: WatchView) -> String {
    match view {
        WatchView::Hex => hex::encode_upper(bytes),
        WatchView::Text => String::from_utf8_lossy(bytes).to_string(),
        WatchView::Binary => {
            let mut out = String::new();
            for (i, b) in bytes.iter().enumerate() {
                if i > 0 { out.push(' '); }
                out.push_str(&format!("{:08b}", b));
            }
            out
        }
    }
}

fn find_message_label(message: &[u8], rules: &[LabelRule]) -> Option<String> {
    for r in rules {
        let start = r.start_index;
        let end = r.end_index;
        if start <= end && end < message.len() {
            let slice = &message[start..=end];
            if slice.len() == r.value.len() && slice == r.value.as_slice() {
                return Some(r.name.clone());
            }
        }
    }
    None
}

fn spawn_connection(address: String) -> (Sender<Vec<u8>>, Receiver<Vec<u8>>, thread::JoinHandle<()>, thread::JoinHandle<()>) {
    let (tx_to_writer, rx_for_writer) = bounded::<Vec<u8>>(1024);
    let (tx_from_reader, rx_from_reader) = bounded::<Vec<u8>>(1024);
    let stream = TcpStream::connect(address.clone()).expect("failed to connect");
    stream
        .set_read_timeout(Some(Duration::from_millis(200)))
        .ok();
    let stream_reader = stream.try_clone().expect("clone stream failed");
    let stream_writer = stream;

    let reader_handle = thread::spawn(move || {
        let mut buf = [0u8; 4096];
        let mut local_stream = stream_reader;
        loop {
            match local_stream.read(&mut buf) {
                Ok(0) => {
                    // connection closed
                    break;
                }
                Ok(n) => {
                    let chunk = buf[..n].to_vec();
                    if tx_from_reader.send(chunk).is_err() {
                        break;
                    }
                }
                Err(_e) => {
                    // timeout or error; just continue polling
                }
            }
        }
    });

    let writer_handle = thread::spawn(move || {
        let mut local_stream = stream_writer;
        loop {
            select! {
                recv(rx_for_writer) -> msg => {
                    match msg {
                        Ok(bytes) => {
                            if let Err(e) = local_stream.write_all(&bytes) {
                                error!("write error: {}", e);
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
                default => { thread::sleep(Duration::from_millis(100)); }
            }
        }
    });

    (tx_to_writer, rx_from_reader, reader_handle, writer_handle)
}

fn frame_messages(buffer: &mut Vec<u8>, start: &[u8], end: &[u8]) -> Vec<Vec<u8>> {
    // Very simple framing: find start then end sequences
    let mut messages = Vec::new();
    loop {
        let start_pos = if start.is_empty() {
            Some(0)
        } else {
            buffer.windows(start.len()).position(|w| w == start)
        };

        let s = match start_pos { Some(p) => p, None => break };
        let after_start = s + start.len();
        if after_start > buffer.len() { break; }

        let end_pos = if end.is_empty() {
            Some(buffer.len())
        } else {
            buffer[after_start..]
                .windows(end.len())
                .position(|w| w == end)
                .map(|p| after_start + p)
        };

        let e = match end_pos { Some(p) => p, None => break };
        let msg_end = e + end.len();
        if msg_end <= buffer.len() {
            messages.push(buffer[s..msg_end].to_vec());
            buffer.drain(0..msg_end);
        } else {
            break;
        }
    }
    messages
}

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
                ui.checkbox(&mut self.state.display_as_text, "Display as text");
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
            } else {
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
            }
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Incoming messages");
                if ui.button("Clear").clicked() {
                    self.state.received_messages.clear();
                    self.incoming_buffer.clear();
                }
            });
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
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
            });
        });

        egui::TopBottomPanel::bottom("bottom").show(ctx, |ui| {
            ui.horizontal(|ui| {
                let reserve_for_button = 100.0; // button + spacing
                let available = ui.available_width();
                let input_width = if available > reserve_for_button { available - reserve_for_button } else { (available * 0.7).max(0.0) };
                ui.add_sized([
                    input_width,
                    0.0,
                ], egui::TextEdit::singleline(&mut self.state.send_hex_input).hint_text("Send hex bytes (e.g. FE ED FA CE)"));
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

fn main() -> eframe::Result<()> {
    env_logger::init();
    let options = eframe::NativeOptions::default();
    eframe::run_native(
        "ByteBuster",
        options,
        Box::new(|_cc| Box::new(ByteBusterApp::default())),
    )
}
