//! Deck legality: a pure function from a format's rules, a deck and the facts
//! about its cards to a structured report. No IO.
//!
//! A deck entry names a *game card* (`card_id`), never a printing: a Standard
//! and a Hyperspace copy of a card are the same card for deckbuilding, and
//! the copy cap counts them together.
//!
//! The rules themselves are data (`formats` / `format_rules` in the catalog);
//! [`FormatRules::from_map`] reads the key/value rows. Nothing here names a
//! format.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::{Deserialize, Serialize};

/// Where an entry sits in a deck.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Zone {
    Main,
    Sideboard,
}

/// One line of a deck: `qty` copies of a game card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeckEntry {
    pub card_id: String,
    pub qty: u32,
    pub zone: Zone,
}

/// A card's standing in one format (`card_legality.status`). A card with no
/// row is legal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Legality {
    #[default]
    Legal,
    Banned,
    /// At most one copy.
    Restricted,
    NotLegal,
}

impl Legality {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "legal" => Self::Legal,
            "banned" => Self::Banned,
            "restricted" => Self::Restricted,
            "not_legal" => Self::NotLegal,
            _ => return None,
        })
    }
}

/// What the checker needs to know about a card, resolved by the caller from
/// the catalog (`cards.card_type`, `cards.aspects`, `card_legality`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CardFacts {
    pub card_type: String,
    pub aspects: Vec<String>,
    pub legality: Legality,
}

impl CardFacts {
    fn is_leader(&self) -> bool {
        self.card_type.eq_ignore_ascii_case("leader")
    }
    fn is_base(&self) -> bool {
        self.card_type.eq_ignore_ascii_case("base")
    }
}

/// A format's deckbuilding rules, as read from `format_rules`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormatRules {
    /// Exactly this many distinct leaders.
    pub leaders: u32,
    /// Exactly this many bases.
    pub bases: u32,
    /// Minimum cards other than leaders and bases, sideboard excluded.
    pub min_deck_size: u32,
    /// Copies of one card allowed (main plus sideboard); `None` = no cap.
    pub max_copies: Option<u32>,
    /// Sideboard size cap; `None` = unbounded.
    pub sideboard_max: Option<u32>,
    /// The leaders' combined aspects must not contain all of these.
    pub leader_aspect_exclusion: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RulesError {
    #[error("format rule {0:?} is missing")]
    Missing(&'static str),
    #[error("format rule {key:?} has the unusable value {value:?}")]
    Bad { key: &'static str, value: String },
}

impl FormatRules {
    /// Build from `format_rules` rows. Every key is required — a missing rule
    /// is an error, never a default (a silent default is a legal-looking deck
    /// under the wrong rules).
    pub fn from_map(m: &BTreeMap<String, String>) -> Result<Self, RulesError> {
        fn get<'a>(
            m: &'a BTreeMap<String, String>,
            k: &'static str,
        ) -> Result<&'a str, RulesError> {
            m.get(k).map(String::as_str).ok_or(RulesError::Missing(k))
        }
        fn num(m: &BTreeMap<String, String>, k: &'static str) -> Result<u32, RulesError> {
            let v = get(m, k)?;
            v.parse().map_err(|_| RulesError::Bad {
                key: k,
                value: v.into(),
            })
        }
        fn cap(m: &BTreeMap<String, String>, k: &'static str) -> Result<Option<u32>, RulesError> {
            match get(m, k)? {
                "none" => Ok(None),
                _ => num(m, k).map(Some),
            }
        }
        Ok(Self {
            leaders: num(m, "leaders")?,
            bases: num(m, "bases")?,
            min_deck_size: num(m, "min_deck_size")?,
            max_copies: cap(m, "max_copies")?,
            sideboard_max: cap(m, "sideboard_max")?,
            leader_aspect_exclusion: get(m, "leader_aspect_exclusion")?
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect(),
        })
    }
}

/// One way a deck breaks its format. Machine-readable; the UI words them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Violation {
    WrongLeaderCount {
        required: u32,
        found: u32,
    },
    DuplicateLeader {
        card_id: String,
    },
    WrongBaseCount {
        required: u32,
        found: u32,
    },
    TooFewCards {
        min: u32,
        found: u32,
    },
    TooManyCopies {
        card_id: String,
        max: u32,
        found: u32,
    },
    SideboardTooLarge {
        max: u32,
        found: u32,
    },
    LeaderInSideboard {
        card_id: String,
    },
    LeaderAspectConflict {
        aspects: Vec<String>,
    },
    CardNotLegal {
        card_id: String,
        status: Legality,
    },
    /// The caller had no facts for this card (not in the catalog).
    UnknownCard {
        card_id: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeckCounts {
    pub leaders: u32,
    pub bases: u32,
    /// Main-deck cards other than leaders and bases.
    pub main: u32,
    pub sideboard: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegalityReport {
    pub legal: bool,
    pub counts: DeckCounts,
    pub violations: Vec<Violation>,
}

/// Check `entries` against `rules`. Violations come out in a stable order.
pub fn check_deck(
    rules: &FormatRules,
    entries: &[DeckEntry],
    cards: &HashMap<String, CardFacts>,
) -> LegalityReport {
    let mut v = Vec::new();
    let mut counts = DeckCounts {
        leaders: 0,
        bases: 0,
        main: 0,
        sideboard: 0,
    };
    // Copies per card, main + sideboard; ordered for stable output.
    let mut copies: BTreeMap<&str, u32> = BTreeMap::new();
    let mut leader_ids: BTreeMap<&str, u32> = BTreeMap::new();
    let mut leader_aspects: BTreeSet<&str> = BTreeSet::new();
    let mut unknown: BTreeSet<&str> = BTreeSet::new();
    let mut offences: BTreeMap<&str, Legality> = BTreeMap::new();

    for e in entries.iter().filter(|e| e.qty > 0) {
        let Some(facts) = cards.get(&e.card_id) else {
            unknown.insert(&e.card_id);
            continue;
        };
        if facts.legality != Legality::Legal && facts.legality != Legality::Restricted {
            offences.insert(&e.card_id, facts.legality);
        }
        if facts.is_leader() {
            if e.zone == Zone::Sideboard {
                v.push(Violation::LeaderInSideboard {
                    card_id: e.card_id.clone(),
                });
                continue;
            }
            counts.leaders += e.qty;
            *leader_ids.entry(&e.card_id).or_default() += e.qty;
            leader_aspects.extend(facts.aspects.iter().map(String::as_str));
        } else if facts.is_base() {
            if e.zone == Zone::Main {
                counts.bases += e.qty;
            }
        } else {
            match e.zone {
                Zone::Main => counts.main += e.qty,
                Zone::Sideboard => counts.sideboard += e.qty,
            }
            *copies.entry(&e.card_id).or_default() += e.qty;
        }
    }

    if counts.leaders != rules.leaders {
        v.push(Violation::WrongLeaderCount {
            required: rules.leaders,
            found: counts.leaders,
        });
    }
    for (id, n) in &leader_ids {
        if *n > 1 {
            v.push(Violation::DuplicateLeader {
                card_id: (*id).into(),
            });
        }
    }
    if counts.bases != rules.bases {
        v.push(Violation::WrongBaseCount {
            required: rules.bases,
            found: counts.bases,
        });
    }
    if counts.main < rules.min_deck_size {
        v.push(Violation::TooFewCards {
            min: rules.min_deck_size,
            found: counts.main,
        });
    }
    if let Some(max) = rules.sideboard_max
        && counts.sideboard > max
    {
        v.push(Violation::SideboardTooLarge {
            max,
            found: counts.sideboard,
        });
    }
    for (id, found) in &copies {
        let restricted = cards
            .get(*id)
            .is_some_and(|c| c.legality == Legality::Restricted);
        let cap = match (rules.max_copies, restricted) {
            (Some(m), true) => Some(m.min(1)),
            (None, true) => Some(1),
            (m, false) => m,
        };
        if let Some(max) = cap
            && *found > max
        {
            v.push(Violation::TooManyCopies {
                card_id: (*id).into(),
                max,
                found: *found,
            });
        }
    }
    if !rules.leader_aspect_exclusion.is_empty()
        && rules
            .leader_aspect_exclusion
            .iter()
            .all(|a| leader_aspects.contains(a.as_str()))
    {
        v.push(Violation::LeaderAspectConflict {
            aspects: rules.leader_aspect_exclusion.clone(),
        });
    }
    for (id, status) in offences {
        v.push(Violation::CardNotLegal {
            card_id: id.into(),
            status,
        });
    }
    for id in unknown {
        v.push(Violation::UnknownCard { card_id: id.into() });
    }
    LegalityReport {
        legal: v.is_empty(),
        counts,
        violations: v,
    }
}

/// Owned and missing copies of one deck line. A deck shows these; it does not
/// reserve physical copies, so the same owned card can be "owned" by two decks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnedLine {
    pub card_id: String,
    pub needed: u32,
    pub owned: u32,
    pub missing: u32,
}

/// `owned` is the collection's count of each game card across all printings.
pub fn owned_missing(entries: &[DeckEntry], owned: &HashMap<String, u32>) -> Vec<OwnedLine> {
    let mut needed: BTreeMap<&str, u32> = BTreeMap::new();
    for e in entries {
        *needed.entry(&e.card_id).or_default() += e.qty;
    }
    needed
        .into_iter()
        .map(|(id, n)| {
            let have = owned.get(id).copied().unwrap_or(0);
            OwnedLine {
                card_id: id.into(),
                needed: n,
                owned: have,
                missing: n.saturating_sub(have),
            }
        })
        .collect()
}
