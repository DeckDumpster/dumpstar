//! `dumpstar-ci-deny --deny-list FILE --form nextest|libtest`: one argument per line on
//! stdout for `deploy/ci.sh` to hand its test runner; the ids left out are named on stderr.

use std::process::ExitCode;

const USAGE: &str = "usage: dumpstar-ci-deny --deny-list FILE --form nextest|libtest";

#[derive(Clone, Copy)]
enum Form {
    Nextest,
    Libtest,
}

fn arguments(args: &[String]) -> Option<(&str, Form)> {
    let (mut file, mut form) = (None, None);
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        let value = it.next()?.as_str();
        match (flag.as_str(), value) {
            ("--deny-list", _) if file.is_none() => file = Some(value),
            ("--form", "nextest") if form.is_none() => form = Some(Form::Nextest),
            ("--form", "libtest") if form.is_none() => form = Some(Form::Libtest),
            _ => return None,
        }
    }
    Some((file?, form?))
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some((file, form)) = arguments(&args) else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let parsed = std::fs::read_to_string(file)
        .map_err(|e| e.to_string())
        .and_then(|text| dumpstar_ci_deny::parse(&text));
    let deny = match parsed {
        Ok(deny) => deny,
        Err(e) => {
            eprintln!("dumpstar-ci-deny: {file}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let lines = match form {
        Form::Nextest => deny.nextest_args(),
        Form::Libtest => deny.libtest_skips(),
    };
    for line in lines {
        println!("{line}");
    }
    let ids = deny.ids();
    let left_out = if ids.is_empty() {
        "left out none".to_string()
    } else {
        format!("left out {} test(s): {}", ids.len(), ids.join(" "))
    };
    eprintln!("deny list at={}: {left_out}", deny.at());
    ExitCode::SUCCESS
}
