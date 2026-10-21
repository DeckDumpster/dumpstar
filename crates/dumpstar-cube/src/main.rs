//! `swu-cube-check allocate [FILE]` plans copy allocation across the whole cube.
//! `swu-cube-check pack|cube [--planned] [FILE...]` (several FILEs only for `cube`, one pack each) — reads the document from FILE or stdin, prints one line
//! per failure, and exits 1 when any rule fails (2 when the input cannot be read).

use std::io::Read;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let planned = args.iter().any(|a| a == "--planned");
    args.retain(|a| a != "--planned");
    let (mode, paths) = match args.as_slice() {
        [m] => (m.as_str(), vec!["-"]),
        [m, p] => (m.as_str(), vec![p.as_str()]),
        [m, ps @ ..] if m == "cube" => (m.as_str(), ps.iter().map(String::as_str).collect()),
        _ => return usage(),
    };
    if mode == "allocate" {
        return allocate(paths[0]);
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
    let mut value = if paths.len() > 1 {
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

fn allocate(path: &str) -> ExitCode {
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
        Ok(plan) if plan.failures.is_empty() => {
            for (card, packs) in &plan.reservations {
                println!("reserve {card}: {}", packs.join(", "));
            }
            println!("ok");
            ExitCode::SUCCESS
        }
        Ok(plan) => {
            for f in &plan.failures {
                println!("FAIL {f}");
            }
            ExitCode::from(1)
        }
    }
}

fn usage() -> ExitCode {
    eprintln!(
        "usage: swu-cube-check pack [--planned] [FILE] | cube [--planned] [FILE...] | allocate [FILE]   (FILE or stdin: the deck JSON, see SKILL.md; cube takes one pack file per deck)"
    );
    ExitCode::from(2)
}
