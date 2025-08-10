use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::{Duration, Instant};

fn build_frame(id: u8, start_bytes: [u8; 2], end_bytes: [u8; 2], msg: &[u8]) -> Vec<u8> {
    let mut frame = vec![0u8; 35];
    frame[0] = start_bytes[0];
    frame[1] = start_bytes[1];
    frame[2] = 0xEE; // garbage
    frame[3] = id;   // id
    for i in 4..10 { frame[i] = (0xA0 + (i as u8)) as u8; }
    let mut msg_buf = [b' '; 21];
    for (i, b) in msg.iter().take(21).enumerate() { msg_buf[i] = *b; }
    frame[10..=30].copy_from_slice(&msg_buf);
    frame[31] = 0xF1;
    frame[32] = 0xF2;
    frame[33] = end_bytes[0];
    frame[34] = end_bytes[1];
    frame
}

fn handle_client(mut stream: TcpStream) -> std::io::Result<()> {
    // Frame spec:
    // [0]   = 0xAA
    // [1]   = 0x55
    // [2]   = garbage
    // [3]   = id (increments)
    // [4..10] = garbage (6 bytes)
    // [10..=30] = 21 bytes ASCII message
    // [31..=32] = garbage (2 bytes)
    // [33..=34] = 0x0D 0x0A

    let start_bytes = [0xAAu8, 0x55u8];
    let end_bytes = [0x0Du8, 0x0Au8];
    let id_cycle: [u8; 3] = [0x01, 0x02, 0x03];
    let mut id_idx: usize = 0;
    let messages = [
        b"PING FROM SERVER......".as_ref(),
        b"DATA-REQUEST FROM SRV".as_ref(),
        b"DATA-RESPONSE FROMSV".as_ref(),
    ];

    // For trigger-based immediate response
    let trigger: [u8; 4] = [0xFE, 0xED, 0xFA, 0xCE]; // FE ED FA CE
    stream.set_read_timeout(Some(Duration::from_millis(100))).ok();
    let mut incoming_buf: Vec<u8> = Vec::new();
    let mut last_periodic = Instant::now();

    loop {
        // Read for trigger
        let mut buf = [0u8; 1024];
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                incoming_buf.extend_from_slice(&buf[..n]);
                while let Some(pos) = incoming_buf.windows(trigger.len()).position(|w| w == trigger) {
                    let drain_end = pos + trigger.len();
                    incoming_buf.drain(0..drain_end);
                    // Send a burst of 3 frames immediately (back-to-back)
                    let burst_ids = [0x01u8, 0x02u8, 0x03u8];
                    let mut out = Vec::with_capacity(35 * burst_ids.len());
                    for bid in burst_ids {
                        let m_idx = (bid.saturating_sub(1)) as usize % messages.len();
                        out.extend_from_slice(&build_frame(bid, start_bytes, end_bytes, messages[m_idx]));
                    }
                    let _ = stream.write_all(&out);
                    let _ = stream.flush();
                }
            }
            Err(_) => {}
        }

        // Periodic frame every 30 seconds
        if last_periodic.elapsed() >= Duration::from_secs(30) {
            let id = id_cycle[id_idx];
            let msg = messages[id_idx];
            let frame = build_frame(id, start_bytes, end_bytes, msg);
            let _ = stream.write_all(&frame);
            let _ = stream.flush();
            id_idx = (id_idx + 1) % id_cycle.len();
            last_periodic = Instant::now();
        }

        thread::sleep(Duration::from_millis(10));
    }

    Ok(())
}

fn main() -> std::io::Result<()> {
    let listener = TcpListener::bind("127.0.0.1:9000")?;
    println!("byte_buster_server listening on 127.0.0.1:9000");
    for incoming in listener.incoming() {
        match incoming {
            Ok(stream) => {
                println!("client connected: {}", stream.peer_addr().unwrap());
                thread::spawn(|| {
                    let _ = handle_client(stream);
                });
            }
            Err(e) => eprintln!("accept error: {}", e),
        }
    }
    Ok(())
}


