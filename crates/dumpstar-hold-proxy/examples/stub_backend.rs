//! A stand-in backend for tests/deploy/hold_proxy_test.sh: answers every request `200 ok`.
//! `stub_backend <port>`; the test stops and starts it the way a deploy restarts the container.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;

fn main() {
    let port = std::env::args().nth(1).expect("usage: stub_backend <port>");
    let listener = TcpListener::bind(format!("127.0.0.1:{port}")).expect("bind");
    for mut conn in listener.incoming().flatten() {
        thread::spawn(move || {
            let mut buf = [0u8; 4096];
            let _ = conn.read(&mut buf);
            let _ = conn.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
        });
    }
}
