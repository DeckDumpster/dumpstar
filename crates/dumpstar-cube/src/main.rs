//! `swu-cube-check pack|cube [--planned] [FILE]` — reads the document from FILE or stdin, prints one line
//! per failure, and exits 1 when any rule fails (2 when the input cannot be read).

use std::io::Read;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let planned = args.iter().any(|a| a == "--planned");
    args.retain(|a| a != "--planned");
    let (mode, path) = match args.as_slice() {
        [m] => (m.as_str(), "-"),
        [m, p] => (m.as_str(), p.as_str()),
        _ => return usage(),
    };
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
    let mut value: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("not JSON: {e}");
            return ExitCode::from(2);
        }
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

fn usage() -> ExitCode {
    eprintln!(
        "usage: swu-cube-check pack|cube [--planned] [FILE]   (FILE or stdin: the deck JSON, see SKILL.md)"
    );
    ExitCode::from(2)
}
