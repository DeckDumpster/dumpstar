//! The SWU product category of each set. The official API states none, so the
//! mapping is declared in `set_categories.tsv` and checked by the tests below
//! against the production set list (`fixtures/swu/production-sets.txt`).

/// Categories in display order: main expansions first.
pub const CATEGORY_ORDER: &[&str] = &[
    "Main Expansions",
    "Weekly Play Promos",
    "Yearly Promos",
    "Convention Exclusives",
    "Judge Program",
    "Intro & Starter Products",
    "Retail & Partner Exclusives",
];

fn table() -> impl Iterator<Item = (&'static str, &'static str)> {
    include_str!("set_categories.tsv")
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.split_once('\t'))
}

/// The category of a set, or `None` for a set the mapping does not list.
pub fn category_for(set_code: &str) -> Option<&'static str> {
    table().find(|(c, _)| *c == set_code).map(|(_, cat)| cat)
}

/// Position of a category in [`CATEGORY_ORDER`]; no category sorts last.
pub fn category_rank(category: Option<&str>) -> usize {
    category
        .and_then(|c| CATEGORY_ORDER.iter().position(|o| *o == c))
        .unwrap_or(CATEGORY_ORDER.len())
}

/// The codes among `codes` that have no category.
pub fn unmapped<'a>(codes: impl IntoIterator<Item = &'a str>) -> Vec<&'a str> {
    codes
        .into_iter()
        .filter(|c| category_for(c).is_none())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_production_set_has_a_category() {
        let codes: Vec<&str> = include_str!("../../../fixtures/swu/production-sets.txt")
            .lines()
            .filter(|l| !l.is_empty())
            .collect();
        assert_eq!(codes.len(), 28);
        assert_eq!(unmapped(codes.iter().copied()), Vec::<&str>::new());
    }

    #[test]
    fn a_set_with_no_category_is_reported() {
        assert_eq!(unmapped(["SOR", "ZZZ9"]), ["ZZZ9"]);
        assert_eq!(category_for("ZZZ9"), None);
    }

    #[test]
    fn every_mapped_category_is_ordered_and_main_expansions_lead() {
        for (code, cat) in table() {
            assert!(
                CATEGORY_ORDER.contains(&cat),
                "{code}: unknown category {cat}"
            );
        }
        assert_eq!(CATEGORY_ORDER[0], "Main Expansions");
        assert_eq!(category_for("SOR"), Some("Main Expansions"));
        assert_eq!(category_for("JTLP"), Some("Weekly Play Promos"));
    }
}
