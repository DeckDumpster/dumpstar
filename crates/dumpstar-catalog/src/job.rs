//! The timer job: fetch, land, then apply. A systemd timer runs this through
//! `dumpstar catalog-apply`; nothing in the request path calls it.

use std::path::Path;

use chrono::{DateTime, Utc};

use crate::Catalog;
use crate::apply::{ApplyReport, apply};
use crate::error::Result;
use crate::landing::{Landed, land};
use crate::source::CatalogSource;

#[derive(Debug)]
pub struct JobReport {
    pub landed: Landed,
    pub applied: ApplyReport,
}

/// Fetch every payload of `source`, land each under `raw_root` (before any
/// parse), then apply each to the catalog at `shared_db`. The only files
/// touched are `raw_root` and the catalog.
pub fn run_once(
    source: &dyn CatalogSource,
    raw_root: &Path,
    shared_db: &Path,
    now: DateTime<Utc>,
) -> Result<Vec<JobReport>> {
    let landed: Vec<(Landed, Vec<u8>)> = source
        .fetch()?
        .into_iter()
        .map(|p| land(raw_root, &p, now).map(|l| (l, p.bytes)))
        .collect::<Result<_>>()?;
    let mut catalog = Catalog::open_writer(shared_db)?;
    let mut out = Vec::new();
    for (landed, bytes) in landed {
        let applied = apply(&mut catalog, &bytes, &landed.provenance)?;
        out.push(JobReport { landed, applied });
    }
    Ok(out)
}
