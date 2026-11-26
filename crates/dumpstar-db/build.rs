//! Compiles every `data/precons/*.json` decklist into the crate, so adding a
//! deck is adding a file. Writes `$OUT_DIR/precons.rs`, which `src/precons.rs`
//! includes: `PRECON_FILES`, `(repo-relative path, contents)` sorted by path.

use std::path::PathBuf;

const DIR: &str = "data/precons";

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let dir = manifest.join("../..").join(DIR);
    println!("cargo:rerun-if-changed=../../{DIR}");

    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| name.ends_with(".json"))
        .collect();
    names.sort();

    let mut out = String::from("pub(crate) const PRECON_FILES: &[(&str, &str)] = &[\n");
    for name in names {
        let path = format!("{DIR}/{name}");
        let from_manifest = format!("/../../{path}");
        out.push_str(&format!(
            "    ({path:?}, include_str!(concat!(env!(\"CARGO_MANIFEST_DIR\"), {from_manifest:?}))),\n"
        ));
    }
    out.push_str("];\n");
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    std::fs::write(out_dir.join("precons.rs"), out).unwrap();
}
