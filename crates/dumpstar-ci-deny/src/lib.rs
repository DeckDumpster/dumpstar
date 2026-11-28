//! Spira's test-registry deny list, as `test-registry deny-list` prints it, turned into the
//! arguments `deploy/ci.sh` leaves tests out with (sp-kn9imf.1). The row grammar and its
//! refusals are `spira-config`'s `DenyList::parse`; the two argument forms are its
//! `nextest_filter` and the harness's libtest `--skip`.

#[cfg(test)]
#[macro_use]
extern crate spira_test_id;

#[cfg(test)]
mod stamped;

const HEADER: &str = "# test-registry deny-list";

/// One parsed deny list.
#[derive(Debug, Default, PartialEq)]
pub struct Deny {
    at: u64,
    ids: Vec<String>,
}

/// Parse a deny list. Only a list that opens with its header and holds exactly the rows the
/// header counts is an answer: an empty or cut-off text is refused, never read as "deny nothing".
pub fn parse(text: &str) -> Result<Deny, String> {
    let mut lines = text
        .lines()
        .enumerate()
        .map(|(i, line)| (i + 1, line))
        .filter(|(_, line)| !line.trim().is_empty());
    let no_header = format!("deny list has no `{HEADER}` header");
    let Some((head_line, head)) = lines.next() else {
        return Err(no_header);
    };
    let Some(fields) = head.strip_prefix(HEADER) else {
        return Err(format!("{no_header} (line {head_line})"));
    };
    let (at, rows) = header_fields(fields).ok_or_else(|| {
        format!("deny list line {head_line}: the header is not `{HEADER} at=<epoch> rows=<n>`")
    })?;
    let mut ids = Vec::new();
    let mut found = 0;
    for (n, line) in lines.filter(|(_, line)| !line.starts_with('#')) {
        ids.extend(row_id(line).map_err(|e| format!("deny list line {n}: {e}"))?);
        found += 1;
    }
    if found != rows {
        return Err(format!(
            "deny list line {head_line}: the header says rows={rows}, but {found} follow"
        ));
    }
    Ok(Deny { at, ids })
}

fn header_fields(fields: &str) -> Option<(u64, usize)> {
    let mut words = fields.split_whitespace();
    let at = words.next()?.strip_prefix("at=")?.parse().ok()?;
    let rows = words.next()?.strip_prefix("rows=")?.parse().ok()?;
    words.next().is_none().then_some((at, rows))
}

/// One `<state>\t<test>\t<bead>\t<until>` row: the id of a `test:` row, nothing for a `suite:` one.
fn row_id(line: &str) -> Result<Option<String>, String> {
    let f: Vec<&str> = line.split('\t').collect();
    if f.len() != 4 {
        return Err(format!("want 4 tab-separated fields, got {}", f.len()));
    }
    if f[0] != "disabled" && f[0] != "quarantined" {
        return Err(format!(
            "state {:?} is neither disabled nor quarantined",
            f[0]
        ));
    }
    let word = |w: &str| {
        !w.is_empty()
            && w.chars()
                .all(|c| c.is_ascii_alphanumeric() || "_:.-/".contains(c))
    };
    let is_id = |id: &str| {
        id.len() == 7
            && id.starts_with('t')
            && id[1..]
                .bytes()
                .all(|b| b.is_ascii_digit() || b.is_ascii_lowercase())
    };
    match f[1].split_once(':') {
        Some(("suite", name)) if word(name) => Ok(None),
        Some(("test", id)) if is_id(id) => Ok(Some(id.to_string())),
        _ => Err(format!("test {:?} is not suite:<file> or test:<id>", f[1])),
    }
}

impl Deny {
    /// The epoch the list was cut at.
    pub fn at(&self) -> u64 {
        self.at
    }

    /// The ids of the `test:` rows, either state. A `suite:` row names a shell suite of
    /// spira's and means nothing here.
    pub fn ids(&self) -> &[String] {
        &self.ids
    }

    /// `cargo nextest run`'s arguments; none when no test is denied.
    pub fn nextest_args(&self) -> Vec<String> {
        if self.ids.is_empty() {
            return Vec::new();
        }
        let terms: Vec<String> = self
            .ids
            .iter()
            .map(|id| format!("test(/::{id}$/)"))
            .collect();
        vec!["-E".to_string(), format!("not ({})", terms.join(" | "))]
    }

    /// The libtest arguments (after `cargo test … --`).
    pub fn libtest_skips(&self) -> Vec<String> {
        self.ids
            .iter()
            .flat_map(|id| ["--skip".to_string(), format!("::{id}")])
            .collect()
    }
}

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

/// What one invocation of the binary exits with and prints.
#[derive(Debug, PartialEq)]
pub struct Outcome {
    pub code: u8,
    pub stdout: String,
    pub stderr: String,
}

/// The `dumpstar-ci-deny` command: `args` without the program name, `read` for the named file.
/// Stdout is one runner argument per line; stderr names the ids left out, for the CI log.
pub fn run(args: &[String], read: impl FnOnce(&str) -> Result<String, String>) -> Outcome {
    let refused = |code, why: String| Outcome {
        code,
        stdout: String::new(),
        stderr: format!("{why}\n"),
    };
    let Some((file, form)) = arguments(args) else {
        return refused(2, USAGE.to_string());
    };
    let deny = match read(file).and_then(|text| parse(&text)) {
        Ok(deny) => deny,
        Err(e) => return refused(1, format!("dumpstar-ci-deny: {file}: {e}")),
    };
    let lines = match form {
        Form::Nextest => deny.nextest_args(),
        Form::Libtest => deny.libtest_skips(),
    };
    let ids = deny.ids();
    let left_out = if ids.is_empty() {
        "left out none".to_string()
    } else {
        format!("left out {} test(s): {}", ids.len(), ids.join(" "))
    };
    Outcome {
        code: 0,
        stdout: lines.iter().map(|line| format!("{line}\n")).collect(),
        stderr: format!("deny list at={}: {left_out}\n", deny.at()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEAD1: &str = "# test-registry deny-list at=1795800000 rows=1\n";

    fn refusal(text: &str) -> String {
        parse(text).expect_err(text)
    }

    #[test_id(tz7ahj1)]
    #[test]
    fn disabled_and_quarantined_tests_are_left_out_of_both_runners() {
        let d = parse(
            "# test-registry deny-list at=1795800000 rows=2\n\
             disabled\ttest:t0aaaaa\tsp-x\t-\n\
             quarantined\ttest:t0bbbbb\tsp-y\t1795900000\n",
        )
        .unwrap();
        assert_eq!(
            d.nextest_args(),
            ["-E", "not (test(/::t0aaaaa$/) | test(/::t0bbbbb$/))"]
        );
        assert_eq!(
            d.libtest_skips(),
            ["--skip", "::t0aaaaa", "--skip", "::t0bbbbb"]
        );
        assert_eq!(d.ids(), ["t0aaaaa", "t0bbbbb"]);
        assert_eq!(d.at(), 1795800000);
    }

    #[test_id(t7a6uvc)]
    #[test]
    fn a_list_with_no_rows_adds_no_argument() {
        let d = parse("# test-registry deny-list at=7 rows=0\n").unwrap();
        assert_eq!(d.at(), 7);
        assert!(d.nextest_args().is_empty());
        assert!(d.libtest_skips().is_empty());
    }

    #[test_id(t7b24o2)]
    #[test]
    fn a_suite_row_is_counted_and_adds_no_argument() {
        let d = parse(&format!("{HEAD1}disabled\tsuite:test-a.sh\tsp-x\t-\n")).unwrap();
        assert_eq!(d.at(), 1795800000);
        assert!(d.nextest_args().is_empty());
        assert!(d.libtest_skips().is_empty());
    }

    #[test_id(tssppum)]
    #[test]
    fn an_empty_text_is_refused_for_having_no_header() {
        for text in ["", "\n\n"] {
            assert_eq!(
                refusal(text),
                "deny list has no `# test-registry deny-list` header"
            );
        }
    }

    #[test_id(topmmmq)]
    #[test]
    fn rows_without_a_header_are_refused_at_the_first_line_that_is_not_one() {
        let e = refusal("\ndisabled\ttest:t0aaaaa\tsp-x\t-\n");
        assert_eq!(
            e,
            "deny list has no `# test-registry deny-list` header (line 2)"
        );
    }

    #[test_id(tup5m97)]
    #[test]
    fn a_header_that_does_not_say_when_and_how_many_is_refused() {
        for head in [
            "# test-registry deny-list rows=0",
            "# test-registry deny-list at=1",
            "# test-registry deny-list at=x rows=0",
        ] {
            let e = refusal(head);
            assert!(e.starts_with("deny list line 1:"), "{e}");
        }
    }

    #[test_id(t2p7m4x)]
    #[test]
    fn a_row_count_the_header_does_not_match_is_refused() {
        for rows in [0, 2] {
            let e = refusal(&format!(
                "# test-registry deny-list at=1 rows={rows}\ndisabled\ttest:t0aaaaa\tsp-x\t-\n"
            ));
            assert_eq!(
                e,
                format!("deny list line 1: the header says rows={rows}, but 1 follow")
            );
        }
    }

    #[test_id(tb5qp1c)]
    #[test]
    fn a_state_that_is_neither_disabled_nor_quarantined_is_refused_by_line() {
        let e = refusal(&format!("{HEAD1}frozen\ttest:t0aaaaa\tsp-x\t-\n"));
        assert_eq!(
            e,
            "deny list line 2: state \"frozen\" is neither disabled nor quarantined"
        );
    }

    #[test_id(ts1j812)]
    #[test]
    fn a_malformed_test_key_is_refused_by_line() {
        for key in [
            "test:T0",
            "test:t0aaaa",
            "test:t0aaaaa$/) | all(",
            "unit:a b::c",
            "suite:a$(id)",
            "suite:",
        ] {
            let e = refusal(&format!("{HEAD1}disabled\t{key}\tsp-x\t-\n"));
            assert_eq!(
                e,
                format!("deny list line 2: test {key:?} is not suite:<file> or test:<id>")
            );
        }
    }

    #[test_id(tf68v0d)]
    #[test]
    fn a_row_without_four_fields_is_refused_by_line() {
        let e = refusal(&format!("{HEAD1}disabled\ttest:t0aaaaa\n"));
        assert_eq!(e, "deny list line 2: want 4 tab-separated fields, got 2");
    }

    const ONE_EACH: &str = "# test-registry deny-list at=9 rows=3\n\
                            disabled\ttest:t0aaaaa\tsp-x\t-\n\
                            disabled\tsuite:test-a.sh\tsp-x\t-\n\
                            quarantined\ttest:t0bbbbb\tsp-y\t-\n";

    fn invoke(args: &str, list: &str) -> Outcome {
        let args: Vec<String> = args.split_whitespace().map(String::from).collect();
        run(&args, |file| {
            assert_eq!(file, "LIST");
            Ok(list.to_string())
        })
    }

    #[test_id(tdzeyp9)]
    #[test]
    fn each_form_prints_one_argument_per_line_and_names_the_ids_left_out() {
        for (form, stdout) in [
            (
                "nextest",
                "-E\nnot (test(/::t0aaaaa$/) | test(/::t0bbbbb$/))\n",
            ),
            ("libtest", "--skip\n::t0aaaaa\n--skip\n::t0bbbbb\n"),
        ] {
            let o = invoke(&format!("--deny-list LIST --form {form}"), ONE_EACH);
            assert_eq!(
                o,
                Outcome {
                    code: 0,
                    stdout: stdout.to_string(),
                    stderr: "deny list at=9: left out 2 test(s): t0aaaaa t0bbbbb\n".to_string(),
                }
            );
        }
    }

    #[test_id(ti3woer)]
    #[test]
    fn a_run_that_leaves_nothing_out_says_so_and_prints_no_argument() {
        let o = invoke(
            "--form nextest --deny-list LIST",
            "# test-registry deny-list at=9 rows=0\n",
        );
        assert_eq!((o.code, o.stdout.as_str()), (0, ""));
        assert_eq!(o.stderr, "deny list at=9: left out none\n");
    }

    #[test_id(ttpg1u9)]
    #[test]
    fn a_malformed_list_exits_1_naming_the_file_and_line_and_prints_no_argument() {
        let o = invoke(
            "--deny-list LIST --form nextest",
            &format!("{HEAD1}frozen\ttest:t0aaaaa\tsp-x\t-\n"),
        );
        assert_eq!((o.code, o.stdout.as_str()), (1, ""));
        assert!(
            o.stderr
                .starts_with("dumpstar-ci-deny: LIST: deny list line 2:"),
            "{}",
            o.stderr
        );
    }

    #[test_id(tvhbttz)]
    #[test]
    fn anything_but_the_two_named_arguments_exits_2_with_usage() {
        for args in [
            "",
            "LIST nextest",
            "--deny-list LIST",
            "--form nextest",
            "--deny-list LIST --form pytest",
            "--deny-list LIST --form nextest --form libtest",
            "--deny-list LIST --form nextest --verbose",
        ] {
            let o = run(
                &args
                    .split_whitespace()
                    .map(String::from)
                    .collect::<Vec<_>>(),
                |_| panic!("usage is settled before the file is read: {args:?}"),
            );
            assert_eq!((o.code, o.stdout.as_str()), (2, ""), "{args:?}");
            assert!(o.stderr.starts_with("usage:"), "{}", o.stderr);
        }
    }
}
