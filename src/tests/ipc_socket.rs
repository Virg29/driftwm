//! The IPC socket under clients that misbehave: one streaming requests is
//! served a read at a time so the loop keeps turning, and a connection past
//! the cap is told so and closed as soon as it is accepted.

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

/// Everything the server sent on `stream` before closing it, or `None` if it
/// kept the connection open.
fn sent_before_close(f: &mut Fixture, stream: &mut UnixStream) -> Option<String> {
    stream
        .set_read_timeout(Some(Duration::from_millis(10)))
        .unwrap();
    let mut sent = Vec::new();
    let mut buf = [0u8; 256];
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        f.pump(1);
        match stream.read(&mut buf) {
            Ok(0) => return Some(String::from_utf8_lossy(&sent).into_owned()),
            Ok(n) => sent.extend_from_slice(&buf[..n]),
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(_) => return Some(String::from_utf8_lossy(&sent).into_owned()),
        }
    }
    None
}

/// Send one request on `stream` and wait for its reply.
fn answered(f: &mut Fixture, stream: &mut UnixStream) -> bool {
    stream.write_all(&request_line()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        f.pump(1);
        if replies_ready(stream) > 0 {
            return true;
        }
    }
    false
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

#[test]
fn connections_past_the_cap_are_dropped_and_a_freed_slot_is_reused() {
    let temp = TempDir::new();
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let cap = 8;
    let ipc = f.start_ipc_with_cap(temp.path(), cap);

    let mut held: Vec<UnixStream> = (0..cap).map(|_| connect(&ipc)).collect();
    f.pump(2);
    assert!(
        answered(&mut f, &mut held[0]),
        "a connection under the cap is served"
    );
    assert!(
        answered(&mut f, &mut held[cap - 1]),
        "the cap's last slot is served"
    );

    let mut over = connect(&ipc);
    let refusal =
        sent_before_close(&mut f, &mut over).expect("a connection past the cap is closed");
    assert!(
        refusal.contains("connections are already open"),
        "it is told why first: {refusal:?}"
    );

    drop(held.pop());
    f.pump(2);
    let mut next = connect(&ipc);
    f.pump(2);
    assert!(
        answered(&mut f, &mut next),
        "a freed slot takes a new connection"
    );
}
