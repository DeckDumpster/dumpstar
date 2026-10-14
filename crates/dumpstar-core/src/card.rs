//! Card-identity helpers.

/// Map a printed collector number to a sortable integer for binder
/// pagination (PLAN.md §3.4).
///
/// Plain numerics — including Hyperspace and Showcase numbers past the set's
/// main run — sort by value. Anything else (a promo such as `P01`) sorts after
/// every plain number, ordered by its digits:
///
/// | Form        | Example   | Result   |
/// |-------------|-----------|----------|
/// | numeric `N` | `193`     | `193`    |
/// | other       | `P01`     | `900001` |
pub fn number_sortable(number: &str) -> i64 {
    let n = number.trim();
    if let Ok(v) = n.parse::<i64>() {
        return v;
    }
    let digits: String = n.chars().filter(char::is_ascii_digit).collect();
    900_000 + digits.parse::<i64>().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::number_sortable;

    #[test]
    fn plain_numbers_sort_by_value() {
        assert_eq!(number_sortable("5"), 5);
        assert_eq!(number_sortable("193"), 193);
        assert_eq!(number_sortable("262"), 262); // Hyperspace
        assert_eq!(number_sortable("500"), 500); // Showcase
        assert_eq!(number_sortable(" 7 "), 7); // whitespace tolerated
    }

    #[test]
    fn non_numeric_forms_sort_after_every_plain_number() {
        assert_eq!(number_sortable("P01"), 900_001);
        assert!(number_sortable("P01") > number_sortable("500"));
        assert!(number_sortable("P02") > number_sortable("P01"));
    }
}
