//! Networking layer: TCP connect and background IO threads.
use crossbeam_channel::{bounded, select, Receiver, Sender};
use log::error;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::thread;
use std::time::Duration;

/// Establish a TCP connection and spawn reader/writer threads.
///
/// Returns `(tx_to_writer, rx_from_reader, reader_join, writer_join)`.
pub fn spawn_connection(address: String) -> (Sender<Vec<u8>>, Receiver<Vec<u8>>, thread::JoinHandle<()>, thread::JoinHandle<()>) {
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
                    break;
                }
                Ok(n) => {
                    let chunk = buf[..n].to_vec();
                    if tx_from_reader.send(chunk).is_err() {
                        break;
                    }
                }
                Err(_e) => {}
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

