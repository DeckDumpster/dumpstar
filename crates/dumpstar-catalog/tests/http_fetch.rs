//! The real HTTP client against a local server: identified User-Agent, gzip
//! accepted and decoded, and the pause between calls honoured.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dumpstar_catalog::fetch::{Fetch, HttpFetch, USER_AGENT};
use flate2::{Compression, write::GzEncoder};

fn serve(n: usize) -> (String, Arc<Mutex<Vec<String>>>, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let h = std::thread::spawn(move || {
        for _ in 0..n {
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            let len = s.read(&mut buf).unwrap();
            let req = String::from_utf8_lossy(&buf[..len]).to_string();
            let path = req.split_whitespace().nth(1).unwrap().to_string();
            log.lock().unwrap().push(req);
            let (body, enc) = if path.starts_with("/gz") {
                let mut e = GzEncoder::new(Vec::new(), Compression::default());
                e.write_all(b"{\"hello\":\"gzip\"}").unwrap();
                (e.finish().unwrap(), "Content-Encoding: gzip\r\n")
            } else {
                (b"plain".to_vec(), "")
            };
            write!(
                s,
                "HTTP/1.1 200 OK\r\n{enc}Content-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            s.write_all(&body).unwrap();
        }
    });
    (base, seen, h)
}

#[test]
fn identifies_itself_accepts_gzip_decodes_it_and_pauses_between_calls() {
    let (base, seen, h) = serve(3);
    let http = HttpFetch::new(Duration::from_millis(250)).unwrap();
    let start = Instant::now();
    assert_eq!(
        http.get(&format!("{base}/gz/page")).unwrap(),
        b"{\"hello\":\"gzip\"}"
    );
    assert_eq!(http.get(&format!("{base}/plain")).unwrap(), b"plain");
    assert_eq!(http.get(&format!("{base}/plain2")).unwrap(), b"plain");
    h.join().unwrap();
    assert!(
        start.elapsed() >= Duration::from_millis(500),
        "two pauses of 250 ms"
    );
    for req in seen.lock().unwrap().iter() {
        let lower = req.to_lowercase();
        assert!(
            lower.contains(&format!("user-agent: {}", USER_AGENT.to_lowercase())),
            "{req}"
        );
        assert!(lower.contains("accept-encoding: gzip"), "{req}");
    }
}
