//! `dumpstar-ci-deny --deny-list FILE --form nextest|libtest`: one argument per line on
//! stdout for `deploy/ci.sh` to hand its test runner; the ids left out are named on stderr.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let outcome = dumpstar_ci_deny::run(&args, |file| {
        std::fs::read_to_string(file).map_err(|e| e.to_string())
    });
    print!("{}", outcome.stdout);
    eprint!("{}", outcome.stderr);
    ExitCode::from(outcome.code)
}
