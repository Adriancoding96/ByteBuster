//! Message framing utilities.
/// Extract framed messages from `buffer` using `start` and `end` delimiters.
pub fn frame_messages(buffer: &mut Vec<u8>, start: &[u8], end: &[u8]) -> Vec<Vec<u8>> {
    let mut messages = Vec::new();
    loop {
        let start_pos = if start.is_empty() { Some(0) } else { buffer.windows(start.len()).position(|w| w == start) };
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
        } else { break; }
    }
    messages
}

