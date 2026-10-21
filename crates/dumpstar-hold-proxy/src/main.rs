//! Hold proxy in front of the dumpstar container (sp-slm2ib, in Rust since sp-io9c7k).
//!
//! Listens on the origin port the Cloudflare tunnel targets and pipes each connection to the
//! container's backend port. A connection that arrives while the backend is down (a deploy
//! restarting it) is HELD, retrying the upstream connect until it answers or `--hold` seconds
//! pass, instead of being refused: the tunnel then sees a slow response, never a 502.
//!
//!     dumpstar-hold-proxy --listen 127.0.0.1:8091 --upstream 127.0.0.1:18091 [--hold 60] [--retry 0.05] [--log FILE]
//!
//! Every held connection is logged (`held <seconds>`) so a deploy can report its window.

use std::fs::OpenOptions;
use std::io::{self, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::process::exit;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

struct Args {
    listen: String,
    upstream: String,
    hold: f64,
    retry: f64,
    log: Option<String>,
}

fn parse_args() -> Result<Args, String> {
    let (mut listen, mut upstream, mut log) = (None, None, None);
    let (mut hold, mut retry) = (60.0, 0.05);
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut value = || it.next().ok_or(format!("{flag} needs a value"));
        match flag.as_str() {
            "--listen" => listen = Some(value()?),
            "--upstream" => upstream = Some(value()?),
            "--log" => log = Some(value()?),
            "--hold" => hold = value()?.parse().map_err(|e| format!("--hold: {e}"))?,
            "--retry" => retry = value()?.parse().map_err(|e| format!("--retry: {e}"))?,
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(Args {
        listen: listen.ok_or("--listen is required")?,
        upstream: upstream.ok_or("--upstream is required")?,
        hold,
        retry,
        log,
    })
}

/// Writes a line to stdout and, when `--log` is given, appends it to that file.
struct Logger(Mutex<Option<std::fs::File>>);

impl Logger {
    fn line(&self, s: &str) {
        let mut out = io::stdout().lock();
        let _ = writeln!(out, "{s}");
        let _ = out.flush();
        if let Some(f) = self.0.lock().unwrap().as_mut() {
            let _ = writeln!(f, "{s}");
        }
    }
}

fn pipe(mut from: TcpStream, mut to: TcpStream) {
    let _ = io::copy(&mut from, &mut to);
    let _ = to.shutdown(Shutdown::Write);
}

fn handle(client: TcpStream, args: &Args, log: &Logger) {
    let start = Instant::now();
    let hold = Duration::from_secs_f64(args.hold);
    let upstream = loop {
        match TcpStream::connect(&args.upstream) {
            Ok(s) => break s,
            Err(_) if start.elapsed() < hold => thread::sleep(Duration::from_secs_f64(args.retry)),
            Err(_) => {
                log.line(&format!("refused after {:.0}s: upstream never answered", args.hold));
                return;
            }
        }
    };
    let waited = start.elapsed().as_secs_f64();
    if waited > 0.2 {
        log.line(&format!("held {waited:.2}s"));
    }
    let (Ok(c2), Ok(u2)) = (client.try_clone(), upstream.try_clone()) else {
        return;
    };
    let back = thread::spawn(move || pipe(u2, c2));
    pipe(client, upstream);
    let _ = back.join();
}

fn main() {
    let args = match parse_args() {
        Ok(a) => Arc::new(a),
        Err(e) => {
            eprintln!("dumpstar-hold-proxy: {e}");
            exit(2);
        }
    };
    let file = args.log.as_ref().map(|p| {
        OpenOptions::new().create(true).append(true).open(p).unwrap_or_else(|e| {
            eprintln!("dumpstar-hold-proxy: --log {p}: {e}");
            exit(2);
        })
    });
    let log = Arc::new(Logger(Mutex::new(file)));
    let listener = TcpListener::bind(&args.listen).unwrap_or_else(|e| {
        eprintln!("dumpstar-hold-proxy: bind {}: {e}", args.listen);
        exit(1);
    });
    log.line(&format!(
        "hold-proxy {} -> {} (hold {:.0}s)",
        args.listen, args.upstream, args.hold
    ));
    for conn in listener.incoming().flatten() {
        let (args, log) = (Arc::clone(&args), Arc::clone(&log));
        thread::spawn(move || handle(conn, &args, &log));
    }
}
