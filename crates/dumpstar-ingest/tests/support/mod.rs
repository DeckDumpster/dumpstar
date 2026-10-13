// The implementation moved to the library behind the `test-support` feature
// so that `dumpstar-cli`'s raw-coverage gate can reach it — a gate that drives
// a whole acquisition phase cannot reach a `tests/` module directly.
pub use dumpstar_ingest::test_upstream::*;
