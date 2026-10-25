//! The IPC socket under clients that misbehave: one streaming requests is
//! served a read at a time so the loop keeps turning.

use std::io::{ErrorKind, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

use super::Fixture;
use super::real::TempDir;
use crate::ipc::protocol::Request;

fn request_line() -> Vec<u8> {
    let mut line = serde_json::to_vec(&Request::Layout { short: false }).unwrap();
    line.push(b'\n');
    line
}

/// Reply lines readable on `stream` right now, without waiting for more.
fn replies_ready(stream: &mut UnixStream) -> usize {
    stream.set_nonblocking(true).unwrap();
    let mut count = 0;
    let mut buf = [0u8; 4096];
    loop {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => count += buf[..n].iter().filter(|&&b| b == b'\n').count(),
            Err(e) if e.kind() == ErrorKind::WouldBlock => break,
            Err(e) => panic!("ipc read error: {e}"),
        }
    }
    count
}

fn connect(ipc: &Path) -> UnixStream {
    UnixStream::connect(ipc).expect("connect ipc socket")
}

#[test]
fn a_client_streaming_requests_is_served_one_read_per_turn_of_the_loop() {
    let temp = TempDir::new();
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let ipc = f.start_ipc(temp.path());

    let mut stream = connect(&ipc);
    let count = 200;
    let burst: Vec<u8> = (0..count).flat_map(|_| request_line()).collect();
    stream.write_all(&burst).unwrap();

    // The first turn that answers anything answers only what one read held.
    let mut first = 0;
    for _ in 0..50 {
        f.pump(1);
        first = replies_ready(&mut stream);
        if first > 0 {
            break;
        }
    }
    assert!(
        (1..count).contains(&first),
        "one turn of the loop answered {first} of {count} requests"
    );

    // Later turns pick up the rest, none stranded behind the read boundary.
    let mut total = first;
    let deadline = Instant::now() + Duration::from_secs(5);
    while total < count && Instant::now() < deadline {
        f.pump(1);
        total += replies_ready(&mut stream);
    }
    assert_eq!(total, count, "every request was answered");
}
