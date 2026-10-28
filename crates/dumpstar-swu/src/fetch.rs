//! The one door to the network. Everything that talks to an upstream goes
//! through [`Fetch`], so a test substitutes recorded bytes and a real run is
//! polite by construction: an identified User-Agent, gzip, a pause between
//! calls, and a bounded retry.

use std::io::Read;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::error::{CatalogError, Result};

pub const USER_AGENT: &str = "dumpstar-swu/0.1 (+https://star.dumpster.cards)";

pub trait Fetch {
    fn get(&self, url: &str) -> Result<Vec<u8>>;
}

pub struct HttpFetch {
    client: reqwest::blocking::Client,
    pause: Duration,
    last: Mutex<Option<Instant>>,
}

impl HttpFetch {
    /// `pause` is the minimum time between two calls on this client.
    pub fn new(pause: Duration) -> Result<Self> {
        let client = reqwest::blocking::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(120))
            .build()
            .map_err(|e| CatalogError::Source(e.to_string()))?;
        Ok(Self {
            client,
            pause,
            last: Mutex::new(None),
        })
    }

    fn wait(&self) {
        let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(t) = *last {
            let since = t.elapsed();
            if since < self.pause {
                std::thread::sleep(self.pause - since);
            }
        }
        *last = Some(Instant::now());
    }

    fn once(&self, url: &str) -> Result<Vec<u8>> {
        self.wait();
        let resp = self
            .client
            .get(url)
            .header("Accept-Encoding", "gzip")
            .send()
            .map_err(|e| CatalogError::Source(format!("{url}: {e}")))?;
        let status = resp.status();
        let gz = resp
            .headers()
            .get("content-encoding")
            .is_some_and(|v| v.as_bytes().eq_ignore_ascii_case(b"gzip"));
        let body = resp
            .bytes()
            .map_err(|e| CatalogError::Source(format!("{url}: {e}")))?;
        if !status.is_success() {
            return Err(CatalogError::Source(format!("{url}: HTTP {status}")));
        }
        if gz {
            let mut out = Vec::new();
            flate2::read::GzDecoder::new(&body[..]).read_to_end(&mut out)?;
            Ok(out)
        } else {
            Ok(body.to_vec())
        }
    }
}

impl Fetch for HttpFetch {
    fn get(&self, url: &str) -> Result<Vec<u8>> {
        let mut err = None;
        for attempt in 0..3u32 {
            match self.once(url) {
                Ok(b) => return Ok(b),
                Err(e) => {
                    err = Some(e);
                    std::thread::sleep(Duration::from_secs(2u64 << attempt));
                }
            }
        }
        Err(err.unwrap())
    }
}
