//! `dumpstar` — Dumpstar command-line entry point.
//!
//! The clap command tree grows as features land (PLAN.md §2, §5). The
//! `ingest` subcommand arrives with a later task.

mod catalog;
mod collection;
mod data;
mod db;
mod export;
mod fixture;
mod import;
mod keys;
mod landing;
mod ops;
mod outbox;
mod serve;
mod setup;
mod tenant;
mod value;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "dumpstar",
    version,
    about = "Dumpstar — a Star Wars: Unlimited collection tracker"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Build the shared catalog database from upstream sources.
    Setup(setup::SetupArgs),
    /// Incremental catalog maintenance (nightly refresh).
    Data(data::DataArgs),
    /// Start the HTTP server.
    Serve(serve::ServeArgs),
    /// Provision the per-tenant collection databases.
    Tenant(tenant::TenantArgs),
    /// Tenant-zone key custody: the master key, and which keys may be derived.
    Keys(keys::KeysArgs),
    /// Build the deterministic test fixture for the intents UI harness.
    SeedFixture(fixture::FixtureArgs),
    /// Database maintenance — snapshot/restore for the UI test harness.
    Db(db::DbArgs),
    /// Run the server's own collection search, read-only, printing rows and (--timing) timings.
    Search(ops::SearchArgs),
    /// Write the collection out in a portable format.
    Export(export::ExportArgs),
    /// Load a portable export back into the collection.
    Import(import::ImportArgs),
    /// Emit current holdings as outbox events — backfill, redrive, DR.
    Outbox(outbox::OutboxArgs),
    /// Apply the SWU catalog (fetch, land raw, apply idempotently); a timer job.
    CatalogApply(catalog::CatalogApplyArgs),
    /// Check the SWU catalog for alarms (missing price day, unmatched printings); a timer job.
    CatalogCheck(catalog::CatalogCheckArgs),
    /// Record a day's collection value for every tenant, offline; exits 2 if any were skipped.
    ValueSnapshots(value::ValueSnapshotsArgs),
}

/// Serialised access to process-wide origin overrides (ENV_TCGCSV_BASE_URL etc.)
/// used by test gates in data.rs and setup.rs. Both modules are compiled into
/// the same test binary, so they need a single lock rather than independent ones
/// that could allow the gates to run concurrently and corrupt each other's ENV.
#[cfg(test)]
pub(crate) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::Setup(args) => setup::run(args),
        Command::Data(args) => data::run(args),
        Command::Serve(args) => serve::run(args),
        Command::Tenant(args) => tenant::run(args),
        Command::Keys(args) => keys::run(args),
        Command::SeedFixture(args) => fixture::run(args),
        Command::Db(args) => db::run(args),
        Command::Search(args) => ops::run_search(args),
        Command::Export(args) => export::run(args),
        Command::Import(args) => import::run(args),
        Command::Outbox(args) => outbox::run(args),
        Command::CatalogApply(args) => catalog::run(args),
        Command::CatalogCheck(args) => catalog::check(args),
        Command::ValueSnapshots(args) => value::run(args),
    }
}
