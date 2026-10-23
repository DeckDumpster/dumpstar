//! Natural ordering for user-named lists: runs of digits compare by value,
//! text compares case-insensitively, so "Pack 2" sorts before "Pack 10".

use std::cmp::Ordering;

/// Compare two names naturally; ties are the caller's to break (by id).
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let run = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut s = String::new();
                    while let Some(c) = it.next_if(char::is_ascii_digit) {
                        s.push(c);
                    }
                    s.trim_start_matches('0').to_string()
                };
                let (da, db) = (run(&mut a), run(&mut b));
                let ord = da.len().cmp(&db.len()).then_with(|| da.cmp(&db));
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            (Some(x), Some(y)) => {
                a.next();
                b.next();
                let ord = x.to_lowercase().cmp(y.to_lowercase());
                if ord != Ordering::Equal {
                    return ord;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digits_compare_by_value() {
        let mut v = vec!["Pack 10", "pack 2", "Pack 1", "Pack 11"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, ["Pack 1", "pack 2", "Pack 10", "Pack 11"]);
    }
}
