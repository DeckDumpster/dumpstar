//! The official Star Wars: Unlimited API (`admin.starwarsunlimited.com`, a
//! Strapi): one page of `/api/cards` in, a [`CatalogPayload`] out.
//!
//! One official entry is one printing: the Standard Foil of a card is a second
//! entry with its own `cardUid`. The `cardUid` is the `printing_id`, so an id
//! survives every later load. The `serialCode` is `SS VV NNN` (set, variant,
//! number); the human-readable variant comes from the `variantTypes` relation.
//!
//! The parser reads relations only when the request populated them: the
//! prescribed query populates `expansion`, `variantTypes` and `rarity`, and a
//! card's type, aspects, traits and art are filled in when a fuller query
//! supplies them. A malformed entry becomes an alarm, never a dropped row.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::error::{CatalogError, Result};
use crate::record::{
    AlarmRecord, CardRecord, CatalogPayload, KeywordRecord, PrintingRecord, RarityRecord, SetRecord,
};

pub const PAGE_SIZE: u32 = 100;

/// Prefix of a card id derived from the name because the API gave none.
pub const PLACEHOLDER: &str = "n:";

pub fn card_name_key(c: &CardRecord) -> String {
    crate::crosswalk::name_key(&format!(
        "{}{}",
        c.name,
        c.subtitle.as_deref().unwrap_or("")
    ))
}

/// Move placeholder-id cards and printings onto the real card whose name key
/// `real` holds. A placeholder with no real card stays as it is.
pub fn remap_cards(payload: &mut CatalogPayload, real: &BTreeMap<String, String>) {
    let moves: BTreeMap<String, String> = payload
        .cards
        .iter()
        .filter(|c| c.card_id.starts_with(PLACEHOLDER))
        .filter_map(|c| Some((c.card_id.clone(), real.get(&card_name_key(c))?.clone())))
        .collect();
    if moves.is_empty() {
        return;
    }
    payload.cards.retain(|c| !moves.contains_key(&c.card_id));
    for p in &mut payload.printings {
        if let Some(r) = moves.get(&p.card_id) {
            p.card_id = r.clone();
        }
    }
}

pub struct Page {
    pub payload: CatalogPayload,
    pub page_count: u32,
    pub entries: usize,
    /// The largest `updatedAt` on the page.
    pub max_updated_at: Option<String>,
}

fn rel<'a>(attrs: &'a Value, name: &str) -> Option<&'a Value> {
    let d = attrs.get(name)?.get("data")?;
    if d.is_null() { None } else { Some(d) }
}

fn rel_name(attrs: &Value, name: &str) -> Option<String> {
    rel(attrs, name)?
        .get("attributes")?
        .get("name")?
        .as_str()
        .map(str::to_string)
}

/// The rarity relation's mark, when the record carries all of it.
fn rarity_mark(attrs: &Value) -> Option<RarityRecord> {
    let r = rel(attrs, "rarity")?.get("attributes")?;
    Some(RarityRecord {
        name: r.get("name")?.as_str()?.to_string(),
        character: r.get("character")?.as_str()?.to_string(),
        color: r.get("color")?.as_str()?.to_string(),
        sort_value: r.get("sortValue")?.as_i64()?,
    })
}

fn rel_names(attrs: &Value, name: &str) -> Vec<String> {
    rel(attrs, name)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|e| e.get("attributes")?.get("name")?.as_str())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Every keyword in the card pool. A keyword the feed names that is not here
/// is an alarm: the importer does not guess at a rule it has not read.
pub const KNOWN_KEYWORDS: &[&str] = &[
    "Ambush",
    "Bounty",
    "Coordinate",
    "Exploit",
    "Grit",
    "Hidden",
    "Overwhelm",
    "Piloting",
    "Plot",
    "Raid",
    "Restore",
    "Saboteur",
    "Sentinel",
    "Shielded",
    "Smuggle",
    "Support",
];

/// The keywords whose rules text always carries a number: `Raid 2`.
const NUMBERED_KEYWORDS: &[&str] = &["Exploit", "Raid", "Restore"];

/// The keyword relation of an entry, each with the number its rules text gives
/// it. The relation names the keywords; the number is read from the text, where
/// the keyword stands at the start of a line or of a comma-separated list.
fn keywords_of(attrs: &Value, card_id: &str, alarms: &mut Vec<AlarmRecord>) -> Vec<KeywordRecord> {
    let rules = ["text", "deployBox", "epicAction"]
        .iter()
        .filter_map(|k| text(attrs, k))
        .collect::<Vec<_>>()
        .join("\n");
    let mut out: Vec<KeywordRecord> = Vec::new();
    for name in rel_names(attrs, "keywords") {
        let Some(known) = KNOWN_KEYWORDS
            .iter()
            .find(|k| k.eq_ignore_ascii_case(&name))
        else {
            alarms.push(AlarmRecord::alarm(
                "official_unparseable_keyword",
                card_id,
                format!("unknown keyword {name:?}"),
            ));
            continue;
        };
        let value = keyword_value(&rules, known);
        if value.is_none() && !rules.is_empty() && NUMBERED_KEYWORDS.contains(known) {
            alarms.push(AlarmRecord::alarm(
                "official_unparseable_keyword",
                card_id,
                format!("{known} has no number in its rules text"),
            ));
        }
        let (granted, conditional) = match standing(&rules, known) {
            Standing::Own { conditional } => (false, conditional),
            Standing::Granted => (true, false),
            Standing::Absent => continue,
        };
        if !out.iter().any(|k| k.name == *known) {
            out.push(KeywordRecord {
                name: (*known).to_string(),
                value,
                granted,
                conditional,
            });
        }
    }
    out
}

/// How a keyword stands on a card (sp-gww4nf): the one rule every card is
/// derived by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Standing {
    /// The card has it, always or, when `conditional`, only under a condition
    /// ("While …, this unit gains X").
    Own { conditional: bool },
    /// The text gives it to another unit ("Attached unit gains X").
    Granted,
    /// The text only takes it away ("All units lose X"): neither.
    Absent,
}

pub(crate) fn standing(rules: &str, name: &str) -> Standing {
    if rules.is_empty() || stands_alone(rules, name) {
        return Standing::Own { conditional: false };
    }
    if has_itself(rules, name) {
        return Standing::Own { conditional: true };
    }
    if only_lost(rules, name) {
        return Standing::Absent;
    }
    Standing::Granted
}

/// Whether every mention of `name` in the text is one being removed ("lose X").
fn only_lost(rules: &str, name: &str) -> bool {
    let mut seen = false;
    let mut from = 0;
    while let Some(at) = rules[from..].find(name) {
        let start = from + at;
        let end = start + name.len();
        from = end;
        let word = rules[..start]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric())
            && !rules[end..].starts_with(|c: char| c.is_alphanumeric());
        if !word {
            continue;
        }
        seen = true;
        let before = rules[..start].to_ascii_lowercase();
        let before = before.trim_end();
        if !(before.ends_with(" lose") || before.ends_with(" loses") || before == "lose") {
            return false;
        }
    }
    seen
}

/// Whether the card has `name` itself: it begins a line or a comma-separated
/// list item of the rules text. A keyword found only mid-sentence ("Attached
/// unit gains Sentinel") is one the card grants (sp-wxj6lr).
pub(crate) fn stands_alone(rules: &str, name: &str) -> bool {
    rules.lines().any(|line| {
        let mut items = line.split(',').map(|i| i.trim_start_matches(['*', ' ']));
        let begins = |item: &str, n: &str| {
            item.strip_prefix(n)
                .is_some_and(|rest| !rest.starts_with(|c: char| c.is_alphanumeric()))
        };
        let Some(first) = items.next() else {
            return false;
        };
        if begins(first, name) {
            return true;
        }
        // Later items count only on a line that opens with a keyword, so the
        // tail of "While …, Sentinel" is not a keyword list.
        KNOWN_KEYWORDS.iter().any(|k| begins(first, k)) && items.any(|i| begins(i, name))
    })
}

/// Whether the card can have `name` itself: it stands alone, or the text gives
/// it to this very unit under a condition ("While you control a Vigilance unit,
/// this unit gains Sentinel"). Only a keyword given to another unit is granted.
pub(crate) fn has_itself(rules: &str, name: &str) -> bool {
    if stands_alone(rules, name) {
        return true;
    }
    let lower = rules.to_ascii_lowercase();
    let name = name.to_ascii_lowercase();
    ["this unit", "this leader"].iter().any(|who| {
        ["gains", "gain", "has", "have"].iter().any(|verb| {
            let needle = format!("{who} {verb} {name}");
            lower.match_indices(&needle).any(|(at, _)| {
                let before_ok = lower[..at]
                    .chars()
                    .next_back()
                    .is_none_or(|c| !c.is_alphanumeric());
                let after_ok =
                    !lower[at + needle.len()..].starts_with(|c: char| c.is_alphanumeric());
                before_ok && after_ok
            })
        })
    })
}

/// The number after `name` where it begins a line or a list item.
fn keyword_value(rules: &str, name: &str) -> Option<i64> {
    line_start_value(rules, name).or_else(|| granted_value(rules, name))
}

/// The number after `name` anywhere in the text, as a keyword granted mid-line:
/// Devotion's "Attached unit gains Restore 2" (sp-8p9a57).
fn granted_value(rules: &str, name: &str) -> Option<i64> {
    let mut from = 0;
    while let Some(at) = rules[from..].find(name) {
        let start = from + at;
        let end = start + name.len();
        let before_ok = rules[..start]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric());
        let rest = rules[end..].strip_prefix(' ').unwrap_or("");
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        if before_ok && !digits.is_empty() {
            return digits.parse().ok();
        }
        from = end;
    }
    None
}

fn line_start_value(rules: &str, name: &str) -> Option<i64> {
    rules
        .lines()
        .flat_map(|l| l.split(','))
        .map(|item| item.trim_start_matches(['*', ' ']))
        .find_map(|item| {
            let rest = item.strip_prefix(name)?;
            let digits: String = rest
                .trim_start()
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            digits.parse().ok()
        })
}

/// The card's rules text: `text`, `deployBox` and `epicAction` as lines, each
/// line once. A leader's deploy box restates its text (Director Krennic's
/// `text` is also the last paragraph of his `deployBox`), so a plain join
/// doubled it. An upgrade's printed stat line is not in `text` at all, only in
/// `upgradePower`/`upgradeHp`, so it is written out (Resilient has no `text`).
fn rules_text_of(attrs: &Value) -> Option<String> {
    let mut lines: Vec<String> = Vec::new();
    let leader = rel_name(attrs, "type").as_deref() == Some("Leader");
    // A leader's front side is `text` and `epicAction`; `deployBox` is the
    // deployed side, set apart by a line of its own (sp-8p9a57).
    let keys: &[&str] = if leader {
        &["text", "epicAction", "deployBox"]
    } else {
        &["text", "deployBox", "epicAction"]
    };
    for key in keys {
        let Some(t) = text(attrs, key) else { continue };
        let mut opened = false;
        for line in t.lines().map(str::trim).filter(|l| !l.is_empty()) {
            let line = tidy_icon_spaces(&strip_markup(line));
            if !lines.contains(&line) {
                if leader && *key == "deployBox" && !opened && !lines.is_empty() {
                    lines.push(DEPLOYED_SIDE.to_string());
                    opened = true;
                }
                lines.push(line);
            }
        }
    }
    if rel_name(attrs, "type").as_deref() == Some("Upgrade") {
        let stat = |k: &str| attrs.get(k).and_then(Value::as_i64);
        let (p, h) = (stat("upgradePower"), stat("upgradeHp"));
        if p.is_some() || h.is_some() {
            let line = format!(
                "Attached unit gets {:+}/{:+}.",
                p.unwrap_or(0),
                h.unwrap_or(0)
            );
            if !lines.iter().any(|l| l.contains(&line)) {
                lines.push(line);
            }
        }
    }
    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}

/// The card's type. The feed files Hardpoint Heavy Blaster (SOR-121) as a Unit
/// with no arena although it reads "Attach to a Vehicle unit": a unit is always
/// in an arena, so an arena-less "Unit" whose text opens "Attach to" is an upgrade.
fn card_type_of(attrs: &Value) -> Option<String> {
    let ty = rel_name(attrs, "type");
    if ty.as_deref() == Some("Unit")
        && rel_names(attrs, "arenas").is_empty()
        && text(attrs, "text").is_some_and(|t| t.trim_start().starts_with("Attach to"))
    {
        return Some("Upgrade".to_string());
    }
    ty
}

/// Rules text arrives with inline markup the printed card draws as an icon:
/// `<uq>` is the Unique mark (AT-ST Raider, ASH-98: "non-<uq> unit"). It reads
/// as the word, and any other tag is dropped.
pub fn strip_markup(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(open) = rest.find('<') {
        let tail = &rest[open..];
        let tag_end = tail.find('>').filter(|_| {
            tail[1..]
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '/')
        });
        out.push_str(&rest[..open]);
        match tag_end {
            Some(end) => {
                if tail[..=end].eq_ignore_ascii_case("<uq>") {
                    out.push_str("Unique");
                }
                rest = &tail[end + 1..];
            }
            None => {
                out.push('<');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// True when `text` still holds a `<tag>` or `</tag>`.
pub fn has_markup(text: &str) -> bool {
    strip_markup(text) != text
}

/// The line that opens a leader's deployed side in `rules_text`.
pub const DEPLOYED_SIDE: &str = "Deployed:";

/// The official text drops an inline icon and keeps the spaces around it:
/// "use the Force . If you do". Close the gap before punctuation and collapse
/// doubled spaces.
fn tidy_icon_spaces(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    for word in line.split_whitespace() {
        let punct = word
            .chars()
            .all(|c| matches!(c, '.' | ',' | ';' | ':' | '!' | '?' | ')'));
        if !out.is_empty() && !punct {
            out.push(' ');
        }
        out.push_str(word);
    }
    out
}

fn text(attrs: &Value, key: &str) -> Option<String> {
    attrs
        .get(key)?
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// A base prints its Epic Action inside `text`, with no `epicAction` of its own
/// (sp-qnkbmh): take the lines from the one that opens with "Epic Action".
fn epic_action_in(attrs: &Value) -> Option<String> {
    let body = text(attrs, "text")?;
    let at = body
        .lines()
        .position(|l| l.trim_start().starts_with("Epic Action"))?;
    Some(body.lines().skip(at).collect::<Vec<_>>().join("\n"))
}

fn image(attrs: &Value) -> Option<String> {
    art(attrs, "artFront")
}

fn back_image(attrs: &Value) -> Option<String> {
    art(attrs, "artBack")
}

fn art(attrs: &Value, key: &str) -> Option<String> {
    let a = rel(attrs, key)?.get("attributes")?;
    a.pointer("/formats/card/url")
        .or_else(|| a.get("url"))?
        .as_str()
        .map(str::to_string)
}

/// Variants the official API files under a main expansion although they are not
/// printed in it: Weekly Play, prerelease and Organized Play prizes. Their
/// numbers are the promo run's own and collide with the set's (`SOR` #1 is
/// Director Krennic and also a Weekly Play Battlefield Marine), so they live in
/// the set's promo code, `<set>P`, as `LOFP`, `SECP` and the rest already do.
fn is_promo_variant(variant: &str) -> bool {
    matches!(
        variant,
        "Weekly Play" | "Prerelease Promo" | "Prerelease Judge" | "Event Exclusive"
    ) || variant.starts_with("SS ")
}

/// The set a printing belongs to: its expansion, or for a promo variant of a
/// main expansion the expansion's promo set.
pub fn promo_home(set_code: &str, variant: &str) -> String {
    let main = crate::set_categories::category_for(set_code) == Some("Main Expansions");
    if main && is_promo_variant(variant) {
        format!("{set_code}P")
    } else {
        set_code.to_string()
    }
}

/// `(set, variant, number)` of a serial code, number without leading zeros.
///
/// A serial is the set's code, two variant characters and the number: `SS VV
/// NNN` for the first sets, and for later ones the set's own code, which is as
/// long as the expansion's (`CST110002`, `C2408001`) or, for promo runs whose
/// code the expansion does not repeat, three or four characters (`i01010077`,
/// `ic27010052`).
pub fn split_serial<'a>(serial: &'a str, set_code: &str) -> Option<(&'a str, &'a str, String)> {
    if !serial.is_ascii() || serial.len() < 5 {
        return None;
    }
    let own = set_code.len();
    let prefix = if own > 2
        && serial
            .get(..own)
            .is_some_and(|p| p.eq_ignore_ascii_case(set_code))
    {
        own
    } else {
        match serial.len() {
            9 => 3,
            10 => 4,
            _ => 2,
        }
    };
    let number = serial[prefix + 2..].trim_start_matches('0');
    let number = if number.is_empty() { "0" } else { number };
    Some((
        &serial[..prefix],
        &serial[prefix..prefix + 2],
        number.to_string(),
    ))
}

pub fn parse_page(bytes: &[u8]) -> Result<Page> {
    let root: Value = serde_json::from_slice(bytes)?;
    let data = root
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| CatalogError::Source("official page has no data array".into()))?;
    let page_count = root
        .pointer("/meta/pagination/pageCount")
        .and_then(Value::as_u64)
        .unwrap_or(1) as u32;
    let mut payload = CatalogPayload::empty();
    let mut max_updated: Option<String> = None;
    let mut seen_cards = std::collections::BTreeSet::new();
    let mut std_cards = std::collections::BTreeSet::new();
    for entry in data {
        let id = entry.get("id").map(|v| v.to_string()).unwrap_or_default();
        let Some(a) = entry.get("attributes") else {
            payload.alarms.push(AlarmRecord::alarm(
                "official_bad_record",
                id,
                "entry has no attributes",
            ));
            continue;
        };
        if let Some(u) = text(a, "updatedAt")
            && max_updated.as_ref().is_none_or(|m| &u > m)
        {
            max_updated = Some(u);
        }
        let (Some(uid), Some(serial)) = (text(a, "cardUid"), text(a, "serialCode")) else {
            payload.alarms.push(AlarmRecord::alarm(
                "official_bad_record",
                id,
                "missing cardUid or serialCode",
            ));
            continue;
        };
        // 856 of the 991 recorded SOR entries carry no cardId (the API leaves it
        // null on the later-created ones). They are placed on their card by name
        // below; until then they carry a placeholder id.
        let card_id = text(a, "cardId").unwrap_or_else(|| {
            format!(
                "{PLACEHOLDER}{}",
                crate::crosswalk::name_key(&format!(
                    "{}{}",
                    text(a, "title").unwrap_or_default(),
                    text(a, "subtitle").unwrap_or_default()
                ))
            )
        });
        let Some(set_code) = rel(a, "expansion")
            .and_then(|e| e.pointer("/attributes/code"))
            .and_then(Value::as_str)
            .map(str::to_string)
        else {
            payload.alarms.push(AlarmRecord::alarm(
                "official_bad_record",
                uid,
                "no expansion",
            ));
            continue;
        };
        let Some((_, variant_code, number)) = split_serial(&serial, &set_code) else {
            payload.alarms.push(AlarmRecord::alarm(
                "official_bad_record",
                uid,
                format!("unreadable serialCode {serial:?}"),
            ));
            continue;
        };
        let vt = rel(a, "variantTypes")
            .and_then(Value::as_array)
            .and_then(|v| v.first())
            .and_then(|v| v.get("attributes"));
        let vt_name = vt
            .and_then(|v| v.get("name"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| {
                if a.get("hyperspace").and_then(Value::as_bool) == Some(true) {
                    "Hyperspace".into()
                } else if a.get("showcase").and_then(Value::as_bool) == Some(true) {
                    "Showcase".into()
                } else {
                    "Standard".into()
                }
            });
        let foil = vt_name.ends_with(" Foil")
            || vt.and_then(|v| v.get("foil")).and_then(Value::as_bool) == Some(true);
        let variant = vt_name
            .strip_suffix(" Foil")
            .unwrap_or(&vt_name)
            .to_string();

        let expansion = set_code.clone();
        let exp = rel(a, "expansion")
            .and_then(|e| e.get("attributes"))
            .unwrap();
        let set_code = promo_home(&set_code, &variant);
        if !payload.sets.iter().any(|s| s.set_code == set_code) {
            let category = crate::set_categories::category_for(&set_code);
            if category.is_none() {
                payload.alarms.push(AlarmRecord::alarm(
                    "unmapped_set_category",
                    set_code.clone(),
                    "set has no product category in data/remedial/set-categories.csv",
                ));
            }
            payload.sets.push(SetRecord {
                set_code: set_code.clone(),
                name: {
                    let name = exp
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or(&set_code)
                        .to_string();
                    if set_code == expansion {
                        name
                    } else {
                        format!("{name} Promos")
                    }
                },
                release_date: None,
                sort_order: exp.get("sortValue").and_then(Value::as_i64).unwrap_or(0),
                category: category.map(str::to_string),
            });
        }

        if seen_cards.insert(card_id.clone())
            || (variant == "Standard" && !foil && std_cards.insert(card_id.clone()))
        {
            let card = CardRecord {
                card_id: card_id.clone(),
                name: text(a, "title").unwrap_or_default(),
                subtitle: text(a, "subtitle"),
                card_type: card_type_of(a),
                cost: a.get("cost").and_then(Value::as_i64),
                power: a.get("power").and_then(Value::as_i64),
                hp: a.get("hp").and_then(Value::as_i64),
                aspects: rel_names(a, "aspects"),
                from_standard: variant == "Standard" && !foil,
                traits: rel_names(a, "traits"),
                arenas: rel_names(a, "arenas"),
                rules_text: rules_text_of(a),
                keywords: keywords_of(a, &card_id, &mut payload.alarms),
                unique: a.get("unique").and_then(Value::as_bool).unwrap_or(false),
                epic_action: text(a, "epicAction").or_else(|| epic_action_in(a)),
                rules_html: {
                    let parts: Vec<String> = ["textStyled", "deployBoxStyled", "epicActionStyled"]
                        .iter()
                        .filter_map(|k| text(a, k))
                        .collect();
                    if parts.is_empty() {
                        None
                    } else {
                        Some(parts.join("\n"))
                    }
                },
            };
            // The Standard non-foil entry defines the card; any other entry
            // only stands in until one is seen.
            payload.cards.retain(|c| c.card_id != card_id);
            payload.cards.push(card);
        }
        if let Some(m) = rarity_mark(a)
            && !payload.rarities.iter().any(|r| r.name == m.name)
        {
            payload.rarities.push(m);
        }
        payload.printings.push(PrintingRecord {
            printing_id: uid.clone(),
            card_id,
            set_code,
            number,
            variant,
            variant_code: variant_code.to_string(),
            finish: if foil { "Foil" } else { "Normal" }.into(),
            rarity: rel_name(a, "rarity"),
            artist: text(a, "artist"),
            image_url: image(a),
            back_image_url: back_image(a),
            official_card_uid: Some(uid),
            tcgcsv_product_id: None,
            tcgcsv_sub_type_name: None,
        });
    }
    let in_page: BTreeMap<String, String> = payload
        .cards
        .iter()
        .filter(|c| !c.card_id.starts_with(PLACEHOLDER))
        .map(|c| (card_name_key(c), c.card_id.clone()))
        .collect();
    remap_cards(&mut payload, &in_page);
    let entries = data.len();
    Ok(Page {
        payload,
        page_count,
        entries,
        max_updated_at: max_updated,
    })
}

#[cfg(test)]
mod granted_keyword_tests {
    use super::{has_itself, stands_alone};

    #[test]
    fn a_conditional_self_keyword_is_the_cards_own() {
        // Shin Hati (ASH-49), Baze Malbus (SOR-65): the card gives itself Sentinel.
        assert!(has_itself(
            "While you control another Aggression unit, this unit gains Sentinel.",
            "Sentinel"
        ));
        assert!(has_itself(
            "When Played: This unit gains Sentinel for this phase.",
            "Sentinel"
        ));
        // Gladiator Star Destroyer (SOR-86), Buy Time (ASH-91): given to another unit.
        assert!(!has_itself(
            "When Played: Give a unit Sentinel.",
            "Sentinel"
        ));
        assert!(!has_itself(
            "Attach to a unit. Attached unit gains Sentinel.",
            "Sentinel"
        ));
        assert!(!has_itself("This unit gains Sentinelfoo.", "Sentinel"));
    }

    #[test]
    fn a_keyword_the_card_gives_is_not_its_own() {
        assert!(!stands_alone("Attached unit gains Sentinel.", "Sentinel"));
        assert!(!stands_alone(
            "Give a unit Sentinel for this phase.",
            "Sentinel"
        ));
        assert!(stands_alone("Sentinel", "Sentinel"));
        assert!(stands_alone(
            "Raid 2, Sentinel\nWhen Played: draw",
            "Sentinel"
        ));
        assert!(stands_alone("Smuggle [3]. Ambush", "Smuggle"));
        assert!(!stands_alone("Sentinelfoo", "Sentinel"));
    }
}
