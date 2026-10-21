//! `swu-cube-check allocate [FILE]` plans copy allocation across the whole cube.
//! `swu-cube-check pack|cube|copies [--planned] [FILE...]` (several FILEs only for `cube` and `copies`, one pack each) — reads the document from FILE or stdin, prints one line
//! per failure, and exits 1 when any rule fails (2 when the input cannot be read).

use std::io::Read;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let planned = args.iter().any(|a| a == "--planned");
    args.retain(|a| a != "--planned");
    let verbose = args.iter().any(|a| a == "--verbose");
    args.retain(|a| a != "--verbose");
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let (mode, paths) = match args.as_slice() {
        [m] => (m.as_str(), vec!["-"]),
        [m, p] => (m.as_str(), vec![p.as_str()]),
        [m, ps @ ..] if m == "cube" || m == "copies" => {
            (m.as_str(), ps.iter().map(String::as_str).collect())
        }
        _ => return usage(),
    };
    if mode == "allocate" {
        return allocate(paths[0], verbose);
    }
    let mut docs = Vec::new();
    for path in &paths {
        let mut text = String::new();
        let read = if *path == "-" {
            std::io::stdin().read_to_string(&mut text).map(|_| ())
        } else {
            std::fs::read_to_string(path).map(|t| text = t)
        };
        if let Err(e) = read {
            eprintln!("cannot read {path}: {e}");
            return ExitCode::from(2);
        }
        match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(v) => {
                let each = match &v {
                    serde_json::Value::Array(a) => a.iter().collect(),
                    one => vec![one],
                };
                for d in each {
                    if let Err(e) = dumpstar_cube::verify_digest(d) {
                        eprintln!("{path}: {e}");
                        return ExitCode::from(2);
                    }
                }
                docs.push(v)
            }
            Err(e) => {
                eprintln!("not JSON: {path}: {e}");
                return ExitCode::from(2);
            }
        }
    }
    // Several files to `cube` are pack documents (or arrays of them), joined into one cube.
    let mut value = if paths.len() > 1
        || (mode == "copies" && docs[0].get("packs").is_none() && !docs[0].is_array())
    {
        let mut packs = Vec::new();
        for d in docs {
            match d {
                serde_json::Value::Array(a) => packs.extend(a),
                other => packs.push(other),
            }
        }
        serde_json::Value::Array(packs)
    } else {
        docs.remove(0)
    };
    if planned {
        dumpstar_cube::strip_reservations(&mut value);
    }
    let result = match mode {
        "pack" => dumpstar_cube::check_pack_value(&value),
        "cube" => dumpstar_cube::check_cube_value(&value),
        "copies" => dumpstar_cube::check_copies_value(&value),
        _ => return usage(),
    };
    match result {
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(2)
        }
        Ok(failures) if failures.is_empty() => {
            println!("ok");
            ExitCode::SUCCESS
        }
        Ok(failures) => {
            for f in &failures {
                println!("FAIL {f}");
            }
            ExitCode::from(1)
        }
    }
}

fn allocate(path: &str, verbose: bool) -> ExitCode {
    let mut text = String::new();
    let read = if path == "-" {
        std::io::stdin().read_to_string(&mut text).map(|_| ())
    } else {
        std::fs::read_to_string(path).map(|t| text = t)
    };
    if let Err(e) = read {
        eprintln!("cannot read {path}: {e}");
        return ExitCode::from(2);
    }
    let plan = serde_json::from_str::<serde_json::Value>(&text)
        .map_err(|e| format!("not JSON: {path}: {e}"))
        .and_then(|v| dumpstar_cube::allocate::allocate_value(&v));
    match plan {
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(2)
        }
        Ok(plan) => {
            if plan.reservations.is_empty() {
                println!("no contested cards");
            }
            for s in &plan.summary {
                let lost: Vec<String> = s
                    .lost
                    .iter()
                    .take(LOST_SHOWN)
                    .map(|(c, to)| format!("{c} to {}", to.join("+")))
                    .collect();
                let more = s.lost.len().saturating_sub(LOST_SHOWN);
                println!(
                    "{}: reserved {}, short {}, lost {}{}",
                    s.name,
                    s.reserved,
                    s.short,
                    s.lost.len(),
                    if lost.is_empty() {
                        String::new()
                    } else {
                        format!(
                            " ({}{})",
                            lost.join(", "),
                            if more > 0 {
                                format!(", +{more} more")
                            } else {
                                String::new()
                            }
                        )
                    }
                );
            }
            if verbose {
                for (card, packs) in &plan.reservations {
                    let (copies, wanting) = plan.contested[card];
                    println!(
                        "reserve {card}: {} (contested: {copies} cop{}, {wanting} packs)",
                        packs.join(", "),
                        if copies == 1 { "y" } else { "ies" }
                    );
                }
            }
            for f in &plan.failures {
                println!("FAIL {f}");
            }
            if plan.failures.is_empty() {
                println!("ok");
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            }
        }
    }
}

/// Cards named per pack in the default summary; `--verbose` has the full reserve lines.
const LOST_SHOWN: usize = 2;

const USAGE: &str = "usage: swu-cube-check pack [--planned] [FILE] | cube [--planned] [FILE...] | copies [--planned] [FILE...] | allocate [--verbose] [FILE]
  pack      check one pack document (get_deck's result) against the pack rules
  cube      check a 20-pack cube: a cube document, or one pack file per deck
  copies    only the copy-count check (cube-copies) over any number of pack files, for a partial build
  allocate  plan copies across the cube (one line per pack: reserved, short, lost; --verbose adds every
            reserve line; a Leader or non-base Rare/Legendary goes to at most one pack): {\"owned\": {id: copies}, \"packs\": [{\"name\", \"slots\": 15,
            \"eligible\": [search_cards rows or card ids], \"picks\": [...], \"candidates\": [...]}]}; only picks and candidates reserve; rows carry type, cost and arenas, so a
            15-slot pack is reserved cards that meet the pack skeleton, or a named shortfall
  FILE is read in place of stdin (`-` is stdin); exit 0 ok, 1 a rule failed, 2 unreadable input";

fn usage() -> ExitCode {
    eprintln!("{USAGE}");
    ExitCode::from(2)
}
