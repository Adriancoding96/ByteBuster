//! Core application state and helpers.
//!
//! This module defines the shared types used across the GUI, networking,
//! and framing layers, along with parsing/formatting helpers.
use crossbeam_channel::{Receiver, Sender};
use std::fmt;

/// How to render watched bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WatchView {
    /// Render as hexadecimal (e.g. `0A FF`).
    Hex,
    /// Render as UTF-8 text (lossy for invalid sequences).
    Text,
    /// Render as space-separated binary octets (e.g. `00001010`).
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

/// Where a watch should apply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WatchTarget {
    /// Apply to all messages.
    All,
    /// Apply only when a message matches the given label.
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

/// A configured item to watch in each message.
#[derive(Clone, Debug)]
pub struct WatchItem {
    /// Display name.
    pub name: String,
    /// Start index (inclusive).
    pub start_index: usize,
    /// End index (inclusive).
    pub end_index: usize,
    /// Rendering preference.
    pub view: WatchView,
    /// Which messages this watch applies to.
    pub target: WatchTarget,
}

/// A rule that assigns a human-friendly label to a message
/// when a slice of its bytes equals the expected value.
#[derive(Clone, Debug)]
pub struct LabelRule {
    /// Label to display when the rule matches.
    pub name: String,
    /// Start index (inclusive).
    pub start_index: usize,
    /// End index (inclusive).
    pub end_index: usize,
    /// Expected byte value for the slice.
    pub value: Vec<u8>,
}

/// Tabs for the left-hand configuration panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeftPanelTab {
    Watch,
    Labels,
}

/// Top-level state for the running app.
pub struct AppState {
    /// Address for the TCP connection.
    pub address_input: String,
    /// Whether a connection is established.
    pub is_connected: bool,
    /// Channel to the background writer thread.
    pub tx_to_writer: Option<Sender<Vec<u8>>>,
    /// Channel receiving chunks from the background reader thread.
    pub rx_from_reader: Option<Receiver<Vec<u8>>>,

    /// Stored recent messages.
    pub received_messages: Vec<Vec<u8>>,
    pub max_messages: usize,
    pub display_as_text: bool,

    /// Start delimiter as space-separated hex (e.g. `AA 55`).
    pub start_pattern: String,
    /// End delimiter as space-separated hex (e.g. `0D 0A`).
    pub end_pattern: String,
    /// Optional data unit size; reserved for future decoding options.
    pub unit_size: usize,

    /// Outgoing bytes to send as space-separated hex.
    pub send_hex_input: String,

    /// Watch items and form state.
    pub watch_items: Vec<WatchItem>,
    pub new_watch_name: String,
    pub new_watch_range: String,
    pub edit_watch_idx: Option<usize>,
    pub edit_watch_name: String,
    pub edit_watch_range: String,
    pub new_watch_view: WatchView,
    pub edit_watch_view: WatchView,
    pub new_watch_target: WatchTarget,
    pub edit_watch_target: WatchTarget,

    /// Message label rules and form state.
    pub label_rules: Vec<LabelRule>,
    pub new_label_name: String,
    pub new_label_range: String,
    pub new_label_value_hex: String,
    pub edit_label_idx: Option<usize>,
    pub edit_label_name: String,
    pub edit_label_range: String,
    pub edit_label_value_hex: String,

    /// Active left panel tab.
    pub left_panel_tab: LeftPanelTab,
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

/// Parse a space-separated hex string into bytes.
pub fn parse_hex_bytes(input: &str) -> Result<Vec<u8>, String> {
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

/// Parse an index or range string (e.g. `3`, `3-5`, `10..20`).
pub fn parse_index_range(input: &str) -> Option<(usize, usize)> {
    let s = input.trim();
    if s.is_empty() { return None; }
    if let Some((a, b)) = s.split_once('-') {
        let start = a.trim().parse::<usize>().ok()?;
        let end = b.trim().parse::<usize>().ok()?;
        Some((start, end))
    } else if let Ok(idx) = s.parse::<usize>() {
        Some((idx, idx))
    } else if let Some((a, b)) = s.split_once("..") { // also support "start..end"
        let start = a.trim().parse::<usize>().ok()?;
        let end = b.trim().parse::<usize>().ok()?;
        Some((start, end))
    } else {
        None
    }
}

/// Render bytes according to a `WatchView`.
pub fn format_bytes_for_view(bytes: &[u8], view: WatchView) -> String {
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

/// Find the first matching label for `message` using `rules`.
pub fn find_message_label(message: &[u8], rules: &[LabelRule]) -> Option<String> {
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

