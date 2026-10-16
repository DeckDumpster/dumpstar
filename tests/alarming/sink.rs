//! A stand-in for healthchecks.io and Pushover: records every request it gets.
//!
//! tests/alarming/run.sh points DUMPSTAR_BACKUP_PING_URL and PUSHOVER_API_URL here so
//! the gate can assert what the alarming layers ACTUALLY SENT, rather than reading
//! the scripts and reasoning about what they would send.
//!
//! One JSON object per request, appended to the log:
//!     {"method": "GET", "path": "/hc/<token>/fail", "body": ""}
//!
//! Std only, built with `rustc` by run.sh. Usage: sink <port> <logfile>

use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;

fn json_str(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn handle(stream: TcpStream, log: &Mutex<std::fs::File>) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();
    let mut length = 0usize;
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h)? == 0 || h.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            if k.eq_ignore_ascii_case("content-length") {
                length = v.trim().parse().unwrap_or(0);
            }
        }
    }
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body)?;
    let rec = format!(
        "{{\"method\": {}, \"path\": {}, \"body\": {}}}\n",
        json_str(&method),
        json_str(&path),
        json_str(&String::from_utf8_lossy(&body))
    );
    {
        let mut f = log.lock().unwrap();
        f.write_all(rec.as_bytes())?;
        f.flush()?;
    }
    let payload = "{\"status\":1}";
    let mut out = stream;
    write!(
        out,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        payload.len(),
        payload
    )
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: sink <port> <logfile>");
        std::process::exit(2);
    }
    let file = OpenOptions::new().create(true).append(true).open(&args[2]).expect("open log");
    let log = Arc::new(Mutex::new(file));
    let listener = TcpListener::bind(("127.0.0.1", args[1].parse().expect("port"))).expect("bind");
    for stream in listener.incoming().flatten() {
        let log = Arc::clone(&log);
        thread::spawn(move || {
            let _ = handle(stream, &log);
        });
    }
}
