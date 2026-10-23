//! The mechanics a card produces, consumes and requires (sp-iz2vmf).
//!
//! Some cards work only if another card supplies a resource or a game state:
//! "use the Force" does nothing unless something earlier said "The Force is
//! with you." The rules text is the only source, so the derivation is one
//! deterministic reading of it, the way `keywords` is derived: a closed
//! vocabulary ([`NAMES`] and `token_unit:<name>`), three roles, and, on a
//! leader, the side the text is printed on.

use serde::{Deserialize, Serialize};

use crate::official::DEPLOYED_SIDE;

pub const PRODUCES: &str = "produces";
pub const CONSUMES: &str = "consumes";
pub const REQUIRES: &str = "requires";

/// The named mechanics. Token units are `token_unit:<name>`, outside this list.
pub const NAMES: &[&str] = &["force", "experience", "shield", "advantage", "initiative"];

/// The tokens that are an upgrade-like marker on a unit, not a unit themselves.
const MARKER_TOKENS: &[&str] = &["experience", "shield", "advantage"];

/// One mechanic of a card. `role` is `produces` (supplies it), `consumes`
/// (spends it) or `requires` (checks it without spending). `optional` is true
/// when the card is still playable without it ("you may use the Force"), false
/// for a cost, where the ability does not exist without it. `side` is a
/// leader's printed side, `front` (undeployed) or `deployed`; other cards have
/// none.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MechanicRecord {
    pub name: String,
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub side: Option<String>,
    #[serde(default)]
    pub optional: bool,
}

/// The mechanics of a card from its rules text. A leader's text is split into
/// sides at the line [`DEPLOYED_SIDE`].
pub fn mechanics_of(card_type: Option<&str>, rules: Option<&str>) -> Vec<MechanicRecord> {
    let leader = card_type == Some("Leader");
    let mut out: Vec<MechanicRecord> = Vec::new();
    let mut side = leader.then_some("front");
    for line in rules.unwrap_or("").lines() {
        if leader && line.trim() == DEPLOYED_SIDE {
            side = Some("deployed");
            continue;
        }
        for (name, role, optional) in scan(&strip_parens(line).to_ascii_lowercase()) {
            let side = side.map(str::to_string);
            match out
                .iter_mut()
                .find(|m| m.name == name && m.role == role && m.side == side)
            {
                Some(m) => m.optional &= optional,
                None => out.push(MechanicRecord {
                    name,
                    role: role.to_string(),
                    side,
                    optional,
                }),
            }
        }
    }
    out
}

/// Reminder text in parentheses ("use the Force (lose your Force token)") is
/// not a second mention.
fn strip_parens(line: &str) -> String {
    let mut depth = 0usize;
    line.chars()
        .filter(|&c| match c {
            '(' => {
                depth += 1;
                false
            }
            ')' => {
                depth = depth.saturating_sub(1);
                false
            }
            _ => depth == 0,
        })
        .collect()
}

fn words(s: &str) -> Vec<&str> {
    s.split(|c: char| !c.is_alphanumeric() && c != '\'')
        .filter(|w| !w.is_empty())
        .collect()
}

/// Every position at which `needle` stands as whole words in `hay`.
fn find_phrase<'a>(hay: &'a str, needle: &'a str) -> impl Iterator<Item = usize> + 'a {
    hay.match_indices(needle).filter_map(move |(at, _)| {
        let before = hay[..at].chars().next_back();
        let after = hay[at + needle.len()..].chars().next();
        let edge = |c: Option<char>| c.is_none_or(|c| !c.is_alphanumeric());
        (edge(before) && edge(after)).then_some(at)
    })
}

/// The mechanics one line (lowercase, reminder text removed) mentions.
fn scan(line: &str) -> Vec<(String, &'static str, bool)> {
    let mut out: Vec<(String, &'static str, bool)> = Vec::new();

    for at in find_phrase(line, "the force is with you") {
        let before = words(&line[..at]);
        let checked = matches!(
            before.last().copied(),
            Some("if" | "while" | "when" | "unless" | "that")
        ) || before.ends_with(&["as", "long", "as"]);
        out.push((
            "force".into(),
            if checked { REQUIRES } else { PRODUCES },
            false,
        ));
    }
    for at in find_phrase(line, "use the force") {
        let optional = words(&line[..at]).last() == Some(&"may");
        out.push(("force".into(), CONSUMES, optional));
    }
    if find_phrase(line, "have the force").next().is_some() {
        out.push(("force".into(), REQUIRES, false));
    }

    for name in MARKER_TOKENS {
        for noun in [format!("{name} token"), format!("{name} tokens")] {
            for at in find_phrase(line, &noun) {
                if let Some(role) = role_before(&line[..at]) {
                    out.push(((*name).into(), role, false));
                }
            }
        }
    }

    for phrase in [
        "take the initiative",
        "takes the initiative",
        "claim the initiative",
        "claims the initiative",
    ] {
        if find_phrase(line, phrase).next().is_some() {
            out.push(("initiative".into(), PRODUCES, false));
        }
    }
    for phrase in [
        "has the initiative",
        "have the initiative",
        "had the initiative",
    ] {
        if find_phrase(line, phrase).next().is_some() {
            out.push(("initiative".into(), REQUIRES, false));
        }
    }

    for at in find_phrase(line, "create").chain(find_phrase(line, "creates")) {
        let ws = words(&line[at..]);
        let Some(end) = ws
            .iter()
            .take(7)
            .position(|w| matches!(*w, "token" | "tokens"))
        else {
            continue;
        };
        let name: Vec<&str> = ws[1..end]
            .iter()
            .copied()
            .skip_while(|w| {
                matches!(*w, "a" | "an" | "one" | "two" | "three" | "four" | "five")
                    || w.chars().all(|c| c.is_ascii_digit())
            })
            .collect();
        if name.is_empty() || name.len() > 3 || MARKER_TOKENS.contains(&name.join(" ").as_str()) {
            continue;
        }
        out.push((format!("token_unit:{}", name.join("_")), PRODUCES, false));
    }
    out
}

/// What the clause before a token mention does with it, by the nearest verb:
/// "give an Experience token" produces it, "a unit with a Shield token"
/// requires it, "remove a Shield token" spends it.
fn role_before(before: &str) -> Option<&'static str> {
    let clause = before
        .rsplit(['.', ':', ';', '\n'])
        .next()
        .unwrap_or(before);
    words(clause).iter().rev().find_map(|w| match *w {
        "give" | "gives" | "giving" | "create" | "creates" => Some(PRODUCES),
        "with" | "has" | "have" | "without" => Some(REQUIRES),
        "remove" | "removes" | "defeat" | "defeats" => Some(CONSUMES),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn of(card_type: &str, rules: &str) -> Vec<(String, String, Option<String>, bool)> {
        mechanics_of(Some(card_type), Some(rules))
            .into_iter()
            .map(|m| (m.name, m.role, m.side, m.optional))
            .collect()
    }

    fn row(
        name: &str,
        role: &str,
        side: Option<&str>,
        optional: bool,
    ) -> (String, String, Option<String>, bool) {
        (name.into(), role.into(), side.map(str::to_string), optional)
    }

    #[test]
    fn a_leader_cost_that_uses_the_force_is_a_non_optional_front_consumer() {
        assert_eq!(
            of(
                "Leader",
                "Action [Exhaust, use the Force]: Attack with a unit.\nDeployed:\nOn Attack: Give a Shield token to this unit."
            ),
            vec![
                row("force", "consumes", Some("front"), false),
                row("shield", "produces", Some("deployed"), false),
            ]
        );
    }

    /// The issue's acceptance: Obi-Wan Kenobi (LOF-8) is force / consumes / front / not optional.
    #[test]
    fn obi_wan_kenobi_lof_8_consumes_the_force_on_his_front_side() {
        let got = mechanics_of(
            Some("Leader"),
            Some(
                "Action [Exhaust, use the Force]: Give a unit +1/+0 for this phase.\nDeployed:\nOn Attack: Draw a card.",
            ),
        );
        assert_eq!(
            got,
            vec![MechanicRecord {
                name: "force".into(),
                role: "consumes".into(),
                side: Some("front".into()),
                optional: false,
            }]
        );
    }

    #[test]
    fn may_use_the_force_is_optional_and_reminder_text_is_not_a_second_mention() {
        assert_eq!(
            of(
                "Unit",
                "When Played: You may use the Force (lose your Force token). If you do, draw a card."
            ),
            vec![row("force", "consumes", None, true)]
        );
    }

    #[test]
    fn the_force_is_with_you_produces_unless_it_is_a_condition() {
        assert_eq!(
            of("Unit", "When Played: The Force is with you."),
            vec![row("force", "produces", None, false)]
        );
        assert_eq!(
            of("Unit", "While the Force is with you, this unit gets +2/+0."),
            vec![row("force", "requires", None, false)]
        );
    }

    #[test]
    fn a_token_is_produced_required_or_spent_by_the_nearest_verb() {
        assert_eq!(
            of(
                "Unit",
                "On Attack: Give an Experience token to a unit with a Shield token on it."
            ),
            vec![
                row("experience", "produces", None, false),
                row("shield", "requires", None, false),
            ]
        );
        assert_eq!(
            of(
                "Unit",
                "When Defeated: Defeat a unit with a Shield token on it."
            ),
            vec![row("shield", "requires", None, false)]
        );
    }

    #[test]
    fn initiative_and_token_units() {
        assert_eq!(
            of(
                "Event",
                "Take the initiative. Create two Battle Droid tokens. If an enemy has the initiative, draw a card."
            ),
            vec![
                row("initiative", "produces", None, false),
                row("initiative", "requires", None, false),
                row("token_unit:battle_droid", "produces", None, false),
            ]
        );
    }

    #[test]
    fn a_card_with_no_mechanic_has_none() {
        assert!(of("Unit", "Sentinel\nRaid 2").is_empty());
        assert!(mechanics_of(Some("Unit"), None).is_empty());
    }
}
