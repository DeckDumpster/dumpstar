//! Card-identity helpers.

/// Map a printed collector number to a sortable integer for binder
/// ordering.
///
/// Plain numerics sort by value, so a Hyperspace or Showcase number (which
/// runs past the set's printed total) falls after the numbered set in its
/// own right. Anything else is a lettered namespace — a promo (`P01`), a
/// token (`T02`), a numbered prize — and sorts after every plain number,
/// ordered by its digits.
///
/// | Form    | Example | Result    |
/// |---------|---------|-----------|
/// | numeric | `262`   | `262`     |
/// | lettered| `P01`   | `1000001` |
/// | lettered| `T02`   | `1000002` |
pub fn number_sort_key(number: &str) -> i64 {
    let n = number.trim();
    if let Ok(v) = n.parse::<i64>() {
        return v;
    }
    let digits: String = n.chars().filter(char::is_ascii_digit).collect();
    1_000_000 + digits.parse::<i64>().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::number_sort_key;

    #[test]
    fn plain_numbers_sort_by_value() {
        assert_eq!(number_sort_key("4"), 4);
        assert_eq!(number_sort_key("262"), 262); // Hyperspace
        assert_eq!(number_sort_key(" 7 "), 7); // whitespace tolerated
    }

    #[test]
    fn lettered_numbers_sort_after_every_plain_one() {
        assert_eq!(number_sort_key("P01"), 1_000_001);
        assert_eq!(number_sort_key("t02"), 1_000_002);
        assert!(number_sort_key("P01") > number_sort_key("999"));
        assert!(number_sort_key("P02") > number_sort_key("P01"));
    }
}
