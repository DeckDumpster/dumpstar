//! The sealed-product image store. The apply step records each sealed product's
//! source picture in `sealed_images`; [`fetch_pending`] downloads the ones not
//! yet stored into a directory the server serves, and stamps the source URL's
//! retrieval date. Pages read the store, never the source.

use std::path::Path;

use crate::Catalog;
use crate::error::Result;
use crate::fetch::Fetch;

/// The outcome of one pass: pictures stored, and one line per product whose
/// picture could not be fetched (retried on the next pass).
#[derive(Debug, Default)]
pub struct ImageReport {
    pub stored: usize,
    pub failed: Vec<String>,
}

impl ImageReport {
    /// A missing picture never fails the sync (the page shows the product
    /// without one); it is reported here and alarmed by `catalog-check`.
    pub fn coverage_line(&self) -> String {
        let mut line = format!(
            "sealed images: stored {}, failed {}",
            self.stored,
            self.failed.len()
        );
        if let Some(first) = self.failed.first() {
            let why = first.rsplit(": ").next().unwrap_or(first);
            line.push_str(&format!(" ({why})"));
        }
        line
    }
}

fn extension(url: &str) -> &str {
    let name = url.split(['?', '#']).next().unwrap_or(url);
    match name.rsplit_once('.').map(|(_, e)| e) {
        Some(e) if matches!(e, "jpg" | "jpeg" | "png" | "webp") => e,
        _ => "jpg",
    }
}

/// Fetch every pending sealed picture into `dir`, writing each atomically.
pub fn fetch_pending(
    catalog: &Catalog,
    http: &dyn Fetch,
    dir: &Path,
    today: &str,
) -> Result<ImageReport> {
    let conn = catalog.conn();
    let pending: Vec<(i64, String)> = conn
        .prepare(
            "SELECT product_id, source_url FROM sealed_images
             WHERE retrieved_on IS NULL ORDER BY product_id",
        )?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<std::result::Result<_, _>>()?;
    let mut report = ImageReport::default();
    if pending.is_empty() {
        return Ok(report);
    }
    std::fs::create_dir_all(dir)?;
    for (id, url) in pending {
        let bytes = match http.get(&url) {
            Ok(b) if !b.is_empty() => b,
            Ok(_) => {
                report.failed.push(format!("{id}: {url}: empty body"));
                continue;
            }
            Err(e) => {
                report.failed.push(format!("{id}: {e}"));
                continue;
            }
        };
        let file = format!("{id}.{}", extension(&url));
        let tmp = dir.join(format!(".{file}.tmp"));
        std::fs::write(&tmp, &bytes)?;
        std::fs::rename(&tmp, dir.join(&file))?;
        conn.execute(
            "UPDATE sealed_images SET retrieved_on = ?1, file = ?2 WHERE product_id = ?3",
            rusqlite::params![today, file, id],
        )?;
        report.stored += 1;
    }
    Ok(report)
}
