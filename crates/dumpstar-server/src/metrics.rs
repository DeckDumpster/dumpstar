//! Performance metrics, pushed as OTLP through `dumpster-kit-telemetry` to the product metrics
//! stack (design: sp-typbus). Per-route request count and latency come from the kit's layer;
//! this module adds the search paging meter that decides whether a deep page needs a
//! materialised sort-key table (sp-fk7n2i: build one only if a deep page exceeds ~200 ms).
//!
//! Attributes are bounded enums only — never a tenant id, an email or the query text.

use dumpster_kit_telemetry::{Config, Histogram, Telemetry};

/// Milliseconds; includes the 200 ms line the dashboard draws.
const MS_BUCKETS: &[f64] = &[
    5.0, 10.0, 25.0, 50.0, 100.0, 200.0, 500.0, 1000.0, 2500.0, 5000.0,
];
const ROW_BUCKETS: &[f64] = &[0.0, 1.0, 10.0, 25.0, 50.0, 100.0, 250.0, 1000.0, 5000.0];

pub const SEARCH_PAGE_METRIC: &str = "dumpstar_search_page_ms";
pub const SEARCH_TOTALS_METRIC: &str = "dumpstar_search_totals_ms";
pub const SEARCH_ROWS_METRIC: &str = "dumpstar_search_rows";

/// The telemetry handle plus the search instruments. `disabled()` records nothing.
#[derive(Clone)]
pub struct Metrics {
    telemetry: Telemetry,
    page_ms: Histogram,
    totals_ms: Histogram,
    rows: Histogram,
}

/// The page index a request lands on, bucketed: `0`, `1`, `2-4`, `5-19`, `20+`.
pub fn depth_bucket(limit: u32, offset: u32) -> &'static str {
    let depth = if limit == 0 { 0 } else { offset / limit };
    match depth {
        0 => "0",
        1 => "1",
        2..=4 => "2-4",
        5..=19 => "5-19",
        _ => "20+",
    }
}

impl Metrics {
    pub fn new(telemetry: Telemetry) -> Self {
        Metrics {
            page_ms: telemetry.histogram(
                SEARCH_PAGE_METRIC,
                "Search row-page query time by page depth, sort and paging mode",
                "ms",
                MS_BUCKETS,
            ),
            totals_ms: telemetry.histogram(
                SEARCH_TOTALS_METRIC,
                "Search count-and-value time, first page only",
                "ms",
                MS_BUCKETS,
            ),
            rows: telemetry.histogram(
                SEARCH_ROWS_METRIC,
                "Rows a search request returned",
                "1",
                ROW_BUCKETS,
            ),
            telemetry,
        }
    }

    #[cfg(test)]
    pub fn disabled() -> Self {
        Self::new(Telemetry::disabled())
    }

    /// Endpoint from `DUMPSTER_OTLP_METRICS_ENDPOINT`; unset is a no-op.
    pub fn from_env() -> Self {
        Self::new(Telemetry::init(Config::from_env(
            "dumpstar",
            "star.dumpster.cards",
            env!("CARGO_PKG_VERSION"),
        )))
    }

    pub fn telemetry(&self) -> &Telemetry {
        &self.telemetry
    }

    /// One served search. `page` is `(limit, offset)`, `None` for `limit=all`. Totals time is
    /// reported only for the first page (or the whole result).
    pub fn search(
        &self,
        sort: Option<&str>,
        page: Option<(u32, u32)>,
        timings: &dumpstar_db::search::SearchTimings,
        rows: usize,
    ) {
        let (bucket, paged) = match page {
            Some((limit, offset)) => (depth_bucket(limit, offset), "offset"),
            None => ("0", "all"),
        };
        let sort = sort.unwrap_or("default");
        self.page_ms.record(
            timings.rows_ms + timings.copies_ms,
            &[
                ("page_depth_bucket", bucket),
                ("sort", sort),
                ("paged", paged),
            ],
        );
        if matches!(page, None | Some((_, 0))) {
            self.totals_ms.record(timings.totals_ms, &[("sort", sort)]);
        }
        self.rows.record(rows as f64, &[("paged", paged)]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn depth_buckets() {
        assert_eq!(depth_bucket(60, 0), "0");
        assert_eq!(depth_bucket(60, 60), "1");
        assert_eq!(depth_bucket(60, 120), "2-4");
        assert_eq!(depth_bucket(60, 60 * 4), "2-4");
        assert_eq!(depth_bucket(60, 60 * 5), "5-19");
        assert_eq!(depth_bucket(60, 60 * 19), "5-19");
        assert_eq!(depth_bucket(60, 60 * 20), "20+");
        assert_eq!(depth_bucket(0, 500), "0");
    }
}
