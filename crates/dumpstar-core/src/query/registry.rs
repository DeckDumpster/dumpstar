//! The keyword registry: the data-driven definition of every search keyword.
//!
//! Per decision D1 (see architecture/SEARCH_QUERY_LANGUAGE.md) keyword
//! definitions are DATA, not code: they live in `data/search_keywords.json`
//! and the `search_keywords` table, and are loaded into a [`KeywordRegistry`]
//! that the pure parser borrows. `dumpstar-core` only *consumes* the registry —
//! the loader that reads the DB/JSON lives in `dumpstar-db` (task idf.6).

use std::collections::HashMap;

/// One keyword definition, as stored in the seed/table.
///
/// The parser only needs `canonical` + `aliases`; the remaining fields are
/// read by the SQL compiler (`dumpstar-db`, task idf.7) and the autocomplete /
/// help surfaces. They are deserialized here so the JSON shape has a single
/// owner.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct KeywordDef {
    /// Canonical internal name, e.g. `card_type`.
    pub canonical: String,
    /// Accepted aliases, e.g. `["t", "type"]`.
    #[serde(default)]
    pub aliases: Vec<String>,
    /// Operators this keyword accepts (`[":", "=", "!="]`).
    #[serde(default)]
    pub operators: Vec<String>,
    /// Value class the compiler dispatches on
    /// (`text`/`numeric`/`enum`/`date`/`json_contains`/`flag`/`modifier`/`collection`).
    #[serde(default)]
    pub kind: String,
    /// Target column or JSON path, e.g. `cards.types`.
    #[serde(default)]
    pub target: Option<String>,
    /// Optional reference to an enum value-set (card types, conditions, …).
    #[serde(default)]
    pub value_enum: Option<String>,
    /// Free-form semantics tag the compiler reads (`superset`/`subset`/`exists`/…).
    #[serde(default)]
    pub semantics: Option<String>,
    /// One-line description for the help page and autocomplete.
    #[serde(default)]
    pub help: Option<String>,
}

impl KeywordDef {
    /// Convenience constructor for tests and code-built registries; metadata
    /// fields default to empty. Production registries come from the seed JSON.
    pub fn new(canonical: impl Into<String>, aliases: &[&str]) -> Self {
        Self {
            canonical: canonical.into(),
            aliases: aliases.iter().map(|s| s.to_string()).collect(),
            operators: Vec::new(),
            kind: String::new(),
            target: None,
            value_enum: None,
            semantics: None,
            help: None,
        }
    }
}

/// An immutable set of keyword definitions with fast alias resolution.
#[derive(Debug, Clone, Default)]
pub struct KeywordRegistry {
    defs: Vec<KeywordDef>,
    /// Lower-cased alias (and canonical) -> canonical name.
    alias_to_canonical: HashMap<String, String>,
}

impl KeywordRegistry {
    /// Build a registry from definitions. The canonical name is always
    /// resolvable as its own alias.
    pub fn new(defs: Vec<KeywordDef>) -> Self {
        let mut alias_to_canonical = HashMap::new();
        for def in &defs {
            alias_to_canonical.insert(def.canonical.to_ascii_lowercase(), def.canonical.clone());
            for alias in &def.aliases {
                alias_to_canonical.insert(alias.to_ascii_lowercase(), def.canonical.clone());
            }
        }
        Self {
            defs,
            alias_to_canonical,
        }
    }

    /// Resolve a raw keyword (as typed) to its canonical name, or `None` if
    /// unknown. Case-insensitive.
    pub fn resolve(&self, raw: &str) -> Option<&str> {
        self.alias_to_canonical
            .get(&raw.to_ascii_lowercase())
            .map(String::as_str)
    }

    /// Look up the full definition for a canonical name.
    pub fn get(&self, canonical: &str) -> Option<&KeywordDef> {
        self.defs.iter().find(|d| d.canonical == canonical)
    }

    /// The valid terms nearest to an unknown `raw`, best first (at most three):
    /// within a small edit distance, or sharing a prefix of three or more
    /// letters with it (`aspec` -> `aspect`).
    pub fn nearest(&self, raw: &str) -> Vec<String> {
        let raw = raw.to_ascii_lowercase();
        let limit = if raw.len() <= 2 { 1 } else { 2 };
        let mut scored: Vec<(usize, String)> = self
            .alias_to_canonical
            .keys()
            .filter_map(|name| {
                let d = edit_distance(&raw, name);
                let related = raw.len() >= 3
                    && (name.starts_with(&raw)
                        || raw.starts_with(name.as_str()) && name.len() >= 3);
                (d <= limit || related).then(|| (d, name.clone()))
            })
            .collect();
        scored.sort();
        scored.into_iter().take(3).map(|(_, n)| n).collect()
    }

    /// All definitions (for autocomplete / help rendering).
    pub fn defs(&self) -> &[KeywordDef] {
        &self.defs
    }

    pub fn is_empty(&self) -> bool {
        self.defs.is_empty()
    }

    pub fn len(&self) -> usize {
        self.defs.len()
    }
}

fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cur = row[j + 1];
            row[j + 1] = (prev + usize::from(ca != *cb)).min(row[j] + 1).min(cur + 1);
            prev = cur;
        }
    }
    row[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_aliases_and_canonical_case_insensitively() {
        let reg = KeywordRegistry::new(vec![KeywordDef::new("card_type", &["t", "type"])]);
        assert_eq!(reg.resolve("t"), Some("card_type"));
        assert_eq!(reg.resolve("TYPE"), Some("card_type"));
        assert_eq!(reg.resolve("card_type"), Some("card_type"));
        assert_eq!(reg.resolve("nope"), None);
    }

    #[test]
    fn nearest_names_a_near_miss() {
        let reg = KeywordRegistry::new(vec![
            KeywordDef::new("aspect", &["a", "aspect"]),
            KeywordDef::new("card_type", &["t", "type"]),
        ]);
        assert_eq!(reg.nearest("aspec"), vec!["aspect"]);
        assert_eq!(reg.nearest("tpye"), vec!["type"]);
        assert!(reg.nearest("zzzzzz").is_empty());
    }
}
