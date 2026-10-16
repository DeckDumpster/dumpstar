//! Raw landing: the bytes of a fetch, stored with their provenance before any
//! parse. Content-addressed, so landing the same bytes twice is one file.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::Result;
use crate::source::RawPayload;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Provenance {
    pub source: String,
    pub url: String,
    pub fetched_at: String,
    pub sha256: String,
    pub bytes: u64,
    #[serde(default)]
    pub as_of: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Landed {
    pub path: PathBuf,
    pub provenance: Provenance,
}

impl Provenance {
    /// What `applied_payloads` is keyed on: the content hash, the day it
    /// describes when the bytes do not say (an unchanged tcgcsv price file is
    /// still a new day's prices), and the source's derivation version when it
    /// is past the first, so a build that derives more from a payload applies
    /// it again (sp-boex8e).
    pub fn applied_key(&self) -> String {
        self.applied_key_at(crate::apply::derivation_version(&self.source))
    }

    pub fn applied_key_at(&self, derivation: u32) -> String {
        let base = match &self.as_of {
            Some(d) => format!("{}@{d}", self.sha256),
            None => self.sha256.clone(),
        };
        if derivation > 1 {
            format!("{base}#d{derivation}")
        } else {
            base
        }
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Write `payload` under `root/<source>/<YYYY-MM-DD>/<sha256>.json`, with a
/// `<sha256>.meta.json` beside it. Both are written to a temp name and renamed,
/// so a crash never leaves a payload without the hash that names it.
pub fn land(root: &Path, payload: &RawPayload, now: DateTime<Utc>) -> Result<Landed> {
    let sha256 = sha256_hex(&payload.bytes);
    let dir = root
        .join(&payload.source)
        .join(now.format("%Y-%m-%d").to_string());
    fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{sha256}.json"));
    let meta_path = dir.join(format!("{sha256}.meta.json"));
    let provenance = Provenance {
        source: payload.source.clone(),
        url: payload.url.clone(),
        fetched_at: now.to_rfc3339(),
        sha256: sha256.clone(),
        bytes: payload.bytes.len() as u64,
        as_of: payload.as_of.clone(),
    };
    if !path.exists() {
        write_atomic(&path, &payload.bytes)?;
    }
    if !meta_path.exists() {
        write_atomic(&meta_path, &serde_json::to_vec_pretty(&provenance)?)?;
    }
    Ok(Landed { path, provenance })
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)?;
    Ok(())
}
