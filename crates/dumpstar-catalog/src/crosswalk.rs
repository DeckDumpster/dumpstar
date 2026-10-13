//! The join between the official catalog and tcgcsv.
//!
//! Official wins every identity, text and art field; tcgcsv supplies a
//! productId and prices. A printing is joined on `(set, variant, number)`, and
//! the match is only kept when the card names agree; a number that points at a
//! different card is left unmatched on both sides and raises an alarm. Printings
//! the number cannot reach (an official Hyperspace leader is numbered inside the
//! leader range, tcgcsv numbers it after the set) are matched by name, and only
//! when exactly one product and one finish-pair qualify.
//!
//! Token backs are handled, not missed. tcgcsv lists a base twice more, as
//! `Capital City // Shield` (number `20 // T02`): those products only describe
//! the reverse side, are never joined, and are counted. A standalone token
//! product (`Experience // Shield`, `T01 // T02`) prices the first token;
//! the official printing of the second (`T02`) is a counted token back.

use std::collections::{BTreeMap, BTreeSet};

use crate::record::AlarmRecord;
use crate::tcgcsv::{GroupInfo, GroupKind, ProductRecord};

/// A printing as the join sees it.
#[derive(Debug, Clone)]
pub struct OfficialPrinting {
    pub printing_id: String,
    pub set_code: String,
    pub number: String,
    pub variant: String,
    pub finish: String,
    pub name: String,
    pub subtitle: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Link {
    pub printing_id: String,
    pub product_id: i64,
    pub sub_type_name: String,
}

#[derive(Debug, Default)]
pub struct Joined {
    pub links: Vec<Link>,
    pub alarms: Vec<AlarmRecord>,
    pub token_backs: usize,
}

/// Variants whose absence from tcgcsv is a fault. Event and prize promos are
/// listed under promo groups whose naming is not predictable; they are counted.
pub const PRICED_VARIANTS: &[&str] = &["Standard", "Hyperspace", "Showcase", "Weekly Play"];

pub fn is_token_number(n: &str) -> bool {
    let b = n.as_bytes();
    b.len() >= 2 && (b[0] == b't' || b[0] == b'T') && b[1].is_ascii_digit()
}

/// A tcgcsv Number like `29 // T02`: a base card listed again for its token side.
pub fn is_token_back_number(number: &str) -> bool {
    let mut parts = number.split("//");
    let first = parts.next().unwrap_or("").trim();
    parts.next().is_some() && !is_token_number(first)
}

fn norm_number(raw: &str) -> String {
    let first = raw
        .split("//")
        .next()
        .unwrap_or(raw)
        .split('/')
        .next()
        .unwrap_or(raw)
        .trim()
        .to_ascii_lowercase();
    let t = first.trim_start_matches('0');
    if t.is_empty() { "0".into() } else { t.into() }
}

/// Latin letters with diacritics fold to their base (the official API writes
/// `Chirrut Îmwe`, tcgcsv `Chirrut Imwe`).
fn fold(c: char) -> char {
    match c {
        'à'..='å' | 'À'..='Å' | 'ā' | 'Ā' | 'ă' | 'ą' => 'a',
        'ç' | 'Ç' | 'ć' | 'č' => 'c',
        'è'..='ë' | 'È'..='Ë' | 'ē' | 'ę' | 'ě' => 'e',
        'ì'..='ï' | 'Ì'..='Ï' | 'ī' => 'i',
        'ñ' | 'Ñ' | 'ń' => 'n',
        'ò'..='ö' | 'Ò'..='Ö' | 'ø' | 'Ø' | 'ō' => 'o',
        'ù'..='ü' | 'Ù'..='Ü' | 'ū' => 'u',
        'ý' | 'ÿ' | 'Ý' => 'y',
        'š' | 'ś' => 's',
        'ž' | 'ź' | 'ż' => 'z',
        c => c,
    }
}

pub fn name_key(s: &str) -> String {
    s.chars()
        .map(fold)
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// `"Mace Windu - Party Crasher (Top 8)"` -> (`"Mace Windu - Party Crasher"`, `"Top 8"`).
fn split_paren(name: &str) -> (&str, &str) {
    let n = name.trim_end();
    if let Some(body) = n.strip_suffix(')')
        && let Some(i) = body.rfind('(')
    {
        return (body[..i].trim_end(), &body[i + 1..]);
    }
    (n, "")
}

/// The parenthetical an official variant's tcgcsv listing carries.
fn expected_qualifiers(variant: &str) -> Vec<String> {
    match variant {
        "SS Top 8" => vec![name_key("Top 8")],
        "SS Top 4" => vec![name_key("Top 4")],
        "SS Finalist" => vec![name_key("Finalist")],
        "SS Champion" => vec![name_key("Champion")],
        "Hyperspace" => vec![name_key("Hyperspace")],
        _ => vec![String::new(), name_key("Judge")],
    }
}

fn strip_variant(name: &str) -> (&str, Option<&str>) {
    for v in ["Hyperspace", "Showcase"] {
        if let Some(rest) = name.strip_suffix(&format!("({v})")) {
            return (rest.trim_end(), Some(v));
        }
    }
    (name, None)
}

type Key = (String, String, String);

enum Class {
    Sealed,
    Ignored,
    Promo {
        group: i64,
        name: String,
        qualifier: String,
    },
    Unmapped(i64),
    TokenBack,
    Card {
        key: Key,
        name: String,
    },
    TokenCard {
        key: Key,
        second: Key,
        name: String,
    },
}

fn classify(p: &ProductRecord, groups: &BTreeMap<i64, GroupInfo>) -> Class {
    let Some(number) = &p.number else {
        return Class::Sealed;
    };
    let Some(info) = groups.get(&p.group_id) else {
        return Class::Unmapped(p.group_id);
    };
    let set = &info.set_code;
    if info.kind == GroupKind::Promo {
        let (base, qual) = split_paren(&p.name);
        return Class::Promo {
            group: p.group_id,
            name: name_key(base),
            qualifier: name_key(qual),
        };
    }
    let (base, suffix) = strip_variant(&p.name);
    let variant = match (info.kind, suffix) {
        (GroupKind::WeeklyPlay, Some("Hyperspace")) => "Hyperspace".to_string(),
        (GroupKind::WeeklyPlay, _) => "Weekly Play".to_string(),
        (GroupKind::HyperspaceOnly, Some("Hyperspace")) => "Hyperspace".to_string(),
        (GroupKind::HyperspaceOnly, _) => return Class::Ignored,
        _ => suffix.unwrap_or("Standard").to_string(),
    };
    let parts: Vec<&str> = number.split("//").collect();
    let first = parts[0].trim();
    let key = |n: &str| (set.clone(), variant.clone(), norm_number(n));
    if parts.len() > 1 {
        if !is_token_number(first) {
            return Class::TokenBack;
        }
        return Class::TokenCard {
            key: key(first),
            second: key(parts[1]),
            name: name_key(base.split(" // ").next().unwrap_or(base)),
        };
    }
    Class::Card {
        key: key(first),
        name: name_key(base),
    }
}

pub fn join(
    printings: &[OfficialPrinting],
    products: &[ProductRecord],
    groups: &BTreeMap<i64, GroupInfo>,
    group_names: &BTreeMap<i64, String>,
) -> Joined {
    let mut out = Joined::default();
    let mut by_key: BTreeMap<Key, Vec<(&ProductRecord, String)>> = BTreeMap::new();
    let mut second_keys: BTreeSet<Key> = BTreeSet::new();
    let mut unmapped: BTreeSet<i64> = BTreeSet::new();
    let mut loose: Vec<(&ProductRecord, Key, String)> = Vec::new();
    let mut promos: Vec<(&ProductRecord, i64, String, String)> = Vec::new();
    for p in products {
        match classify(p, groups) {
            Class::Sealed | Class::Ignored => {}
            Class::Promo {
                group,
                name,
                qualifier,
            } => promos.push((p, group, name, qualifier)),
            Class::Unmapped(g) => {
                unmapped.insert(g);
            }
            Class::TokenBack => out.token_backs += 1,
            Class::Card { key, name } => {
                by_key
                    .entry(key.clone())
                    .or_default()
                    .push((p, name.clone()));
                loose.push((p, key, name));
            }
            Class::TokenCard { key, second, name } => {
                second_keys.insert(second);
                by_key
                    .entry(key.clone())
                    .or_default()
                    .push((p, name.clone()));
                loose.push((p, key, name));
            }
        }
    }
    for g in &unmapped {
        out.alarms.push(AlarmRecord::alarm(
            "unmapped_group",
            g.to_string(),
            format!(
                "tcgcsv group {g} ({}) has card products but maps to no official set",
                group_names.get(g).map(String::as_str).unwrap_or("?")
            ),
        ));
    }
    let sets: BTreeSet<&str> = groups
        .values()
        .map(|g| g.set_code.as_str())
        .filter(|s| *s != "*")
        .collect();

    let mut matched_products: BTreeSet<i64> = BTreeSet::new();
    let mut ambiguous: BTreeSet<i64> = BTreeSet::new();
    let mut linked: BTreeSet<&str> = BTreeSet::new();
    let link = |out: &mut Joined, pr: &OfficialPrinting, product: i64, also_foil: bool| {
        out.links.push(Link {
            printing_id: pr.printing_id.clone(),
            product_id: product,
            sub_type_name: pr.finish.clone(),
        });
        // tcgcsv prices every Showcase and prize promo as Foil; the official API
        // lists each once.
        if (also_foil || pr.variant == "Showcase") && pr.finish == "Normal" {
            out.links.push(Link {
                printing_id: pr.printing_id.clone(),
                product_id: product,
                sub_type_name: "Foil".into(),
            });
        }
    };

    let mut off_by_key: BTreeMap<Key, Vec<&OfficialPrinting>> = BTreeMap::new();
    for pr in printings {
        off_by_key
            .entry((
                pr.set_code.clone(),
                pr.variant.clone(),
                norm_number(&pr.number),
            ))
            .or_default()
            .push(pr);
    }
    for (key, offs) in &off_by_key {
        let Some(cands) = by_key.get(key) else {
            continue;
        };
        if cands.len() > 1 {
            for (p, _) in cands {
                if ambiguous.insert(p.product_id) {
                    out.alarms.push(AlarmRecord::alarm(
                        "ambiguous_tcgcsv",
                        p.product_id.to_string(),
                        format!(
                            "{} products claim {}/{}/{}",
                            cands.len(),
                            key.0,
                            key.1,
                            key.2
                        ),
                    ));
                }
            }
            for o in offs {
                linked.insert(&o.printing_id);
            }
            continue;
        }
        let (p, pname) = &cands[0];
        let ok: Vec<&&OfficialPrinting> = offs
            .iter()
            .filter(|o| {
                name_key(&format!(
                    "{}{}",
                    o.name,
                    o.subtitle.as_deref().unwrap_or("")
                )) == *pname
            })
            .collect();
        if ok.is_empty() {
            continue;
        }
        matched_products.insert(p.product_id);
        for o in ok {
            link(&mut out, o, p.product_id, false);
            linked.insert(&o.printing_id);
        }
    }

    // Second pass: what the number could not reach, matched by name.
    let mut off_by_name: BTreeMap<(String, String, String), Vec<&OfficialPrinting>> =
        BTreeMap::new();
    for pr in printings
        .iter()
        .filter(|p| !linked.contains(p.printing_id.as_str()))
    {
        off_by_name
            .entry((
                pr.set_code.clone(),
                pr.variant.clone(),
                name_key(&format!(
                    "{}{}",
                    pr.name,
                    pr.subtitle.as_deref().unwrap_or("")
                )),
            ))
            .or_default()
            .push(pr);
    }
    let mut prod_by_name: BTreeMap<(String, String, String), Vec<&ProductRecord>> = BTreeMap::new();
    for (p, key, name) in &loose {
        if !matched_products.contains(&p.product_id) && !ambiguous.contains(&p.product_id) {
            prod_by_name
                .entry((key.0.clone(), key.1.clone(), name.clone()))
                .or_default()
                .push(p);
        }
    }
    for (nk, offs) in &off_by_name {
        let Some(ps) = prod_by_name.get(nk) else {
            continue;
        };
        let finishes: BTreeSet<&str> = offs.iter().map(|o| o.finish.as_str()).collect();
        if ps.len() != 1 || finishes.len() != offs.len() {
            continue;
        }
        matched_products.insert(ps[0].product_id);
        for o in offs {
            link(&mut out, o, ps[0].product_id, false);
            linked.insert(&o.printing_id);
        }
    }

    // Third pass: prize and event promos, on name within the groups that list
    // the printing's variant.
    let mut claimed: BTreeSet<(i64, String)> = BTreeSet::new();
    let unlinked: Vec<&OfficialPrinting> = printings
        .iter()
        .filter(|p| !linked.contains(p.printing_id.as_str()))
        .collect();
    for pr in unlinked {
        let name = name_key(&format!(
            "{}{}",
            pr.name,
            pr.subtitle.as_deref().unwrap_or("")
        ));
        let quals = expected_qualifiers(&pr.variant);
        let cands: Vec<&(&ProductRecord, i64, String, String)> = promos
            .iter()
            .filter(|(_, g, n, q)| {
                *n == name
                    && quals.contains(q)
                    && groups
                        .get(g)
                        .is_some_and(|i| i.variants.contains(&pr.variant))
            })
            .collect();
        if cands.len() == 1 && claimed.insert((cands[0].0.product_id, pr.finish.clone())) {
            matched_products.insert(cands[0].0.product_id);
            link(&mut out, pr, cands[0].0.product_id, true);
            linked.insert(&pr.printing_id);
        }
    }
    for (p, ..) in &promos {
        if !matched_products.contains(&p.product_id) {
            out.alarms.push(AlarmRecord::info(
                "tcgcsv_unlisted_promo",
                p.product_id.to_string(),
                format!(
                    "tcgcsv promo {:?} matches no official printing here",
                    p.name
                ),
            ));
        }
    }

    for pr in printings {
        if linked.contains(pr.printing_id.as_str()) || !sets.contains(pr.set_code.as_str()) {
            continue;
        }
        let key = (
            pr.set_code.clone(),
            pr.variant.clone(),
            norm_number(&pr.number),
        );
        if is_token_number(&pr.number) && second_keys.contains(&key) {
            out.token_backs += 1;
            continue;
        }
        let what = format!(
            "{} {} {} {} #{}",
            pr.set_code, pr.variant, pr.finish, pr.name, pr.number
        );
        if PRICED_VARIANTS.contains(&pr.variant.as_str()) && !is_token_number(&pr.number) {
            out.alarms.push(AlarmRecord::alarm(
                "official_without_tcgcsv",
                pr.printing_id.clone(),
                format!("{what}: official printing with no tcgcsv product"),
            ));
        } else {
            out.alarms.push(AlarmRecord::info(
                "official_unlisted",
                pr.printing_id.clone(),
                format!("{what}: not listed by tcgcsv (promo or token)"),
            ));
        }
    }
    for (p, _, _) in &loose {
        if !matched_products.contains(&p.product_id) && !ambiguous.contains(&p.product_id) {
            out.alarms.push(AlarmRecord::alarm(
                "tcgcsv_without_official",
                p.product_id.to_string(),
                format!(
                    "tcgcsv product {} {:?} #{} has no official printing",
                    p.product_id,
                    p.name,
                    p.number.as_deref().unwrap_or("")
                ),
            ));
        }
    }
    out.links.sort();
    out.links.dedup();
    out.alarms.sort();
    out
}
