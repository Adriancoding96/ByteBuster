//! Suspected data rules and evaluation.

use crate::app::state::{parse_hex_bytes, LabelRule, WatchTarget};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExpectedKind {
    Text,
    Hex,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Info,
    Warning,
    Critical,
}

#[derive(Clone, Debug)]
pub struct SuspectRule {
    pub name: String,
    pub start_index: usize,
    pub end_index: usize,
    pub expected_kind: ExpectedKind,
    pub expected_value: String,
    pub target: WatchTarget,
    pub severity: Severity,
}

/// Evaluate suspect rules for a message; return human-readable warnings for non-matches.
pub fn check_suspects_for_message(
    message: &[u8],
    active_label: &Option<String>,
    rules: &[SuspectRule],
) -> Vec<(Severity, String)> {
    let mut warnings = Vec::new();
    for r in rules {
        let applies = match (&r.target, active_label) {
            (WatchTarget::All, _) => true,
            (WatchTarget::Label(t), Some(lbl)) => t == lbl,
            (WatchTarget::Label(_), None) => false,
        };
        if !applies { continue; }
        if r.start_index > r.end_index || r.end_index >= message.len() { continue; }
        let slice = &message[r.start_index..=r.end_index];
        let ok = match r.expected_kind {
            ExpectedKind::Text => {
                let found = String::from_utf8_lossy(slice);
                found == r.expected_value
            }
            ExpectedKind::Hex => {
                if let Ok(exp) = parse_hex_bytes(&r.expected_value) {
                    exp.as_slice() == slice
                } else { false }
            }
        };
        if !ok {
            let got_repr = match r.expected_kind {
                ExpectedKind::Text => String::from_utf8_lossy(slice).to_string(),
                ExpectedKind::Hex => format!("0x{}", hex::encode_upper(slice)),
            };
            warnings.push((
                r.severity,
                format!(
                    "{}: expected {} at [{}..{}], got {}",
                    r.name,
                    match r.expected_kind { ExpectedKind::Text => r.expected_value.clone(), ExpectedKind::Hex => format!("0x{}", r.expected_value) },
                    r.start_index,
                    r.end_index,
                    got_repr
                ),
            ));
        }
    }
    warnings
}


