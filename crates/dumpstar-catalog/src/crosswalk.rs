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
    /// The serial code's variant digits; two printings may share every other
    /// key field and differ only here.
    pub variant_code: String,
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

/// A hand-resolved link (`remedial.tsv`): the printing the automatic join could
/// not place unambiguously, and the TCGplayer product it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remedial {
    pub printing_id: String,
    pub product_id: i64,
    pub source: String,
    pub reason: String,
}

/// The hand-kept remedial crosswalk (`remedial.tsv`).
pub fn remedial_table() -> Vec<Remedial> {
    include_str!("remedial.tsv")
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let mut f = l.split('\t');
            Some(Remedial {
                printing_id: f.next()?.trim().to_string(),
                product_id: f.next()?.trim().parse().ok()?,
                source: f.next()?.trim().to_string(),
                reason: f.next()?.trim().to_string(),
            })
        })
        .collect()
}

/// What the join needs besides the two catalogs.
#[derive(Debug, Default)]
pub struct Extra<'a> {
    /// The price sub types (`Normal`, `Foil`) each product carries.
    pub sub_types: BTreeMap<i64, Vec<String>>,
    pub remedial: &'a [Remedial],
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

const MAIN_FAMILY: &[&str] = &[
    "",
    "foil",
    "hyperspace",
    "hyperspacefoil",
    "prestige",
    "prestigefoil",
    "serialized",
    "showcase",
];

/// The parenthetical(s) TCGplayer puts after a name for an official
/// `(variant, finish)`. A foil of the main run is its own product (`(Foil)`,
/// `(Hyperspace Foil)`, `(Prestige Foil)`), as is every Prestige and Serialized
/// printing. An event variant (`GC Top 8`) is listed by its place alone.
pub fn listing_qualifiers(variant: &str, finish: &str) -> Vec<String> {
    let foil = finish == "Foil";
    let q = |s: &str| vec![name_key(s)];
    match variant {
        "Standard" | "Weekly Play" => q(if foil { "Foil" } else { "" }),
        "Hyperspace" => q(if foil {
            "Hyperspace Foil"
        } else {
            "Hyperspace"
        }),
        "Standard Prestige" => q("Prestige"),
        "Foil Prestige" => q("Prestige Foil"),
        "Serialized Prestige" => q("Serialized"),
        "Showcase" => q("Showcase"),
        v => {
            let mut out = vec![name_key(v), String::new()];
            if let Some((prefix, rest)) = v.split_once(' ')
                && prefix.len() == 2
            {
                out.push(name_key(rest));
            }
            if foil {
                out.push(name_key("Foil"));
            }
            out
        }
    }
}

/// A product as the listing stage sees it: a name and the parenthetical after it.
struct Listing<'a> {
    product: &'a ProductRecord,
    name: String,
    qualifier: String,
    set: Option<&'a str>,
    weekly_play: bool,
    /// The group is of the kind whose products carry no variant parenthetical.
    listed: bool,
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
    Qualified,
    TokenBack {
        key: Key,
        name: String,
    },
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
    // `(Foil)`, `(Prestige)`, `(Costco Exclusive)`: a product of its own with a
    // number of its own, never the printing that number names.
    let (_, qual) = split_paren(&p.name);
    if parts.len() == 1
        && !qual.is_empty()
        && !qual.starts_with(|c: char| c.is_ascii_digit())
        && !matches!(qual, "Hyperspace" | "Showcase")
    {
        return Class::Qualified;
    }
    let key = |n: &str| (set.clone(), variant.clone(), norm_number(n));
    if parts.len() > 1 {
        if !is_token_number(first) {
            return Class::TokenBack {
                key: key(first),
                name: name_key(base.split(" // ").next().unwrap_or(base)),
            };
        }
        return Class::TokenCard {
            key: key(first),
            second: key(parts[1]),
            name: name_key(base.split(" // ").next().unwrap_or(base)),
        };
    }
    let (numbered, qual) = split_paren(base);
    Class::Card {
        key: key(first),
        // `(85/104)`: a second printing of the card, told apart by its number.
        name: name_key(if qual.starts_with(|c: char| c.is_ascii_digit()) {
            numbered
        } else {
            base
        }),
    }
}

pub fn join(
    printings: &[OfficialPrinting],
    products: &[ProductRecord],
    groups: &BTreeMap<i64, GroupInfo>,
    group_names: &BTreeMap<i64, String>,
) -> Joined {
    join_with(printings, products, groups, group_names, &Extra::default())
}

pub fn join_with(
    printings: &[OfficialPrinting],
    products: &[ProductRecord],
    groups: &BTreeMap<i64, GroupInfo>,
    group_names: &BTreeMap<i64, String>,
    extra: &Extra,
) -> Joined {
    let mut out = Joined::default();
    let mut listings: Vec<Listing> = Vec::new();
    let mut qualified: Vec<&ProductRecord> = Vec::new();
    let mut token_backs: BTreeMap<Key, Vec<(&ProductRecord, String)>> = BTreeMap::new();
    let mut by_key: BTreeMap<Key, Vec<(&ProductRecord, String)>> = BTreeMap::new();
    let mut second_keys: BTreeSet<Key> = BTreeSet::new();
    let mut unmapped: BTreeSet<i64> = BTreeSet::new();
    let mut loose: Vec<(&ProductRecord, Key, String)> = Vec::new();
    let mut promos: Vec<(&ProductRecord, i64, String, String)> = Vec::new();
    for p in products {
        let weekly_play = group_names
            .get(&p.group_id)
            .is_some_and(|n| n.contains("Weekly Play"));
        // A Weekly Play promo may be listed with no Number at all.
        if p.number
            .as_deref()
            .map_or(weekly_play, |n| !n.contains("//"))
        {
            let (base, qual) = split_paren(&p.name);
            let info = groups.get(&p.group_id);
            listings.push(Listing {
                product: p,
                name: name_key(base),
                qualifier: name_key(qual),
                set: info.map(|i| i.set_code.as_str()).filter(|s| *s != "*"),
                weekly_play,
                listed: info.is_some_and(|i| i.kind == GroupKind::Listed),
            });
        }
        match classify(p, groups) {
            Class::Sealed | Class::Ignored => {}
            Class::Qualified => qualified.push(p),
            Class::Promo {
                group,
                name,
                qualifier,
            } => promos.push((p, group, name, qualifier)),
            Class::Unmapped(g) => {
                unmapped.insert(g);
            }
            Class::TokenBack { key, name } => {
                out.token_backs += 1;
                token_backs.entry(key).or_default().push((p, name));
            }
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
        // Two products can carry one number (a misprinted `83/64`); the name
        // tells them apart when only one agrees with an official printing.
        let named: Vec<&(&ProductRecord, String)> = cands
            .iter()
            .filter(|(_, n)| {
                offs.iter().any(|o| {
                    name_key(&format!(
                        "{}{}",
                        o.name,
                        o.subtitle.as_deref().unwrap_or("")
                    )) == *n
                })
            })
            .collect();
        let cands: Vec<(&ProductRecord, String)> = if cands.len() > 1 && named.len() == 1 {
            vec![named[0].clone()]
        } else {
            cands.clone()
        };
        if cands.len() > 1 {
            for (p, _) in &cands {
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
        // Two official printings of one finish that tcgcsv lists as one
        // product (they differ only in the serial's variant code): which one
        // the product prices is not knowable here, so it is an alarm, not a
        // guess (sp-5zcm06).
        let mut per_finish: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for o in &ok {
            per_finish
                .entry(o.finish.as_str())
                .or_default()
                .push(o.variant_code.as_str());
        }
        if let Some((finish, codes)) = per_finish.iter().find(|(_, c)| c.len() > 1) {
            if ambiguous.insert(p.product_id) {
                out.alarms.push(AlarmRecord::alarm(
                    "ambiguous_tcgcsv",
                    p.product_id.to_string(),
                    format!(
                        "{} official {finish} printings (variant codes {}) claim tcgcsv product {} at {}/{}/{}",
                        codes.len(),
                        codes.join(", "),
                        p.product_id,
                        key.0,
                        key.1,
                        key.2
                    ),
                ));
            }
            for o in offs {
                linked.insert(&o.printing_id);
            }
            continue;
        }
        matched_products.insert(p.product_id);
        for o in ok {
            link(&mut out, o, p.product_id, false);
            linked.insert(&o.printing_id);
        }
    }

    // A base with a token on its back is sold as one product per token
    // (`Nevarro City, Restored // Mandalorian`, `// Advantage`, `// Shield`) and
    // never alone: each is the same card, so the lowest productId stands for it
    // and the rest stay counted token backs.
    for (key, offs) in &off_by_key {
        let Some(cands) = token_backs.get(key) else {
            continue;
        };
        let ok: Vec<&&OfficialPrinting> = offs
            .iter()
            .filter(|o| !linked.contains(o.printing_id.as_str()))
            .filter(|o| {
                cands.iter().any(|(_, n)| {
                    *n == name_key(&format!(
                        "{}{}",
                        o.name,
                        o.subtitle.as_deref().unwrap_or("")
                    ))
                })
            })
            .collect();
        let finishes: BTreeSet<&str> = ok.iter().map(|o| o.finish.as_str()).collect();
        if ok.is_empty() || finishes.len() != ok.len() {
            continue;
        }
        let Some((p, _)) = cands.iter().min_by_key(|(p, _)| p.product_id) else {
            continue;
        };
        matched_products.insert(p.product_id);
        for o in ok {
            link(&mut out, o, p.product_id, false);
            linked.insert(&o.printing_id);
            out.alarms.push(AlarmRecord::info(
                "base_listed_per_token",
                o.printing_id.clone(),
                format!(
                    "{} {} #{}: sold as {} products, one per token; linked to the lowest, {}",
                    o.set_code,
                    o.name,
                    o.number,
                    cands.len(),
                    p.product_id
                ),
            ));
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

    // Fourth pass: what is listed under a name and a parenthetical rather than
    // a number. TCGplayer files a foil, a Prestige or a Serialized printing as
    // a product of its own, a promo under the group of the set it came from
    // (a Costco Leia under Ashes of the Empire), so the candidates are every
    // group of the printing's own set, plus -- for a variant only a promo has
    // -- every group at all. One candidate, claimed by one printing, or none.
    let sub_types_of = |product: i64, pr: &OfficialPrinting| -> Vec<String> {
        match extra.sub_types.get(&product).map(Vec::as_slice) {
            Some([only]) => vec![only.clone()],
            Some(many) if many.contains(&pr.finish) => vec![pr.finish.clone()],
            Some(many) if !many.is_empty() => many.to_vec(),
            _ => {
                // No price seen yet: the listing says which. A foil, a
                // Serialized, a Showcase or a promo is quoted as Foil.
                let foil = match listings.iter().find(|l| l.product.product_id == product) {
                    Some(l) => !matches!(l.qualifier.as_str(), "" | "hyperspace" | "prestige"),
                    None => pr.finish == "Foil",
                };
                vec![if foil { "Foil" } else { "Normal" }.into()]
            }
        }
    };
    let mut listed: BTreeMap<i64, Vec<&OfficialPrinting>> = BTreeMap::new();
    for pr in printings
        .iter()
        .filter(|p| !linked.contains(p.printing_id.as_str()))
    {
        let name = name_key(&format!(
            "{}{}",
            pr.name,
            pr.subtitle.as_deref().unwrap_or("")
        ));
        let quals = listing_qualifiers(&pr.variant, &pr.finish);
        let weekly = pr.variant == "Weekly Play";
        // TCGplayer lists a Weekly Play promo by its title alone.
        let title = name_key(&pr.name);
        let find = |tier: u8| -> Vec<&Listing> {
            listings
                .iter()
                .filter(|l| {
                    (l.name == name || (weekly && l.name == title))
                        && (quals.contains(&l.qualifier)
                            || (l.listed
                                && l.qualifier.is_empty()
                                && l.set == Some(pr.set_code.as_str())))
                        && l.weekly_play == weekly
                        && !matched_products.contains(&l.product.product_id)
                        && if l.set == Some(pr.set_code.as_str()) {
                            tier == 1
                        } else if tier == 2 {
                            // A prize group shared by many sets, read only when
                            // the printing's own set has nothing by that name.
                            l.set.is_none() && !weekly
                        } else {
                            // Only a name no main-run product carries is a promo's.
                            !l.qualifier.is_empty()
                                && !MAIN_FAMILY.contains(&l.qualifier.as_str())
                                && quals.contains(&l.qualifier)
                        }
                })
                .collect()
        };
        let mut cands = find(1);
        if cands.is_empty() {
            cands = find(2);
        }
        if let [only] = cands.as_slice() {
            listed.entry(only.product.product_id).or_default().push(pr);
        }
    }
    for (product, prs) in &listed {
        let mut per: BTreeMap<Vec<String>, Vec<&OfficialPrinting>> = BTreeMap::new();
        for pr in prs {
            per.entry(sub_types_of(*product, pr)).or_default().push(pr);
        }
        if per.values().any(|v| v.len() > 1) {
            if ambiguous.insert(*product) {
                out.alarms.push(AlarmRecord::alarm(
                    "ambiguous_tcgcsv",
                    product.to_string(),
                    format!(
                        "{} official printings ({}) claim tcgcsv product {product} by name",
                        prs.len(),
                        prs.iter()
                            .map(|p| format!(
                                "{} {} {} #{}",
                                p.set_code, p.variant, p.finish, p.number
                            ))
                            .collect::<Vec<_>>()
                            .join("; ")
                    ),
                ));
            }
            continue;
        }
        matched_products.insert(*product);
        for (subs, v) in per {
            for sub in subs {
                out.links.push(Link {
                    printing_id: v[0].printing_id.clone(),
                    product_id: *product,
                    sub_type_name: sub,
                });
            }
            linked.insert(&v[0].printing_id);
        }
    }

    // The remedial crosswalk: printings no rule above places unambiguously.
    let by_id: BTreeMap<&str, &OfficialPrinting> = printings
        .iter()
        .map(|p| (p.printing_id.as_str(), p))
        .collect();
    let product_ids: BTreeSet<i64> = products.iter().map(|p| p.product_id).collect();
    for r in extra.remedial {
        let Some(pr) = by_id.get(r.printing_id.as_str()) else {
            continue;
        };
        if !product_ids.contains(&r.product_id) {
            out.alarms.push(AlarmRecord::alarm(
                "remedial_stale",
                r.printing_id.clone(),
                format!(
                    "remedial crosswalk names tcgcsv product {} which tcgcsv no longer lists ({})",
                    r.product_id, r.reason
                ),
            ));
            continue;
        }
        if linked.contains(pr.printing_id.as_str()) {
            let same = out
                .links
                .iter()
                .any(|l| l.printing_id == r.printing_id && l.product_id == r.product_id);
            if !same {
                out.alarms.push(AlarmRecord::alarm(
                    "remedial_conflict",
                    r.printing_id.clone(),
                    format!(
                        "remedial crosswalk says product {} but the join placed this printing elsewhere",
                        r.product_id
                    ),
                ));
            }
            continue;
        }
        for sub in sub_types_of(r.product_id, pr) {
            out.links.push(Link {
                printing_id: r.printing_id.clone(),
                product_id: r.product_id,
                sub_type_name: sub,
            });
        }
        matched_products.insert(r.product_id);
        ambiguous.remove(&r.product_id);
        linked.insert(&pr.printing_id);
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
    for p in &qualified {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn printing(uid: &str, code: &str) -> OfficialPrinting {
        OfficialPrinting {
            printing_id: uid.into(),
            set_code: "SEC".into(),
            number: "1125".into(),
            variant: "Standard".into(),
            variant_code: code.into(),
            finish: "Normal".into(),
            name: "Senator Chuchi".into(),
            subtitle: None,
        }
    }

    /// sp-5zcm06: two official printings sharing set, number, display variant
    /// and finish, and one tcgcsv product for them: an alarm, not a guess.
    #[test]
    fn one_product_for_two_printings_of_one_finish_is_ambiguous_not_guessed() {
        let groups = BTreeMap::from([(
            1,
            GroupInfo {
                set_code: "SEC".into(),
                kind: GroupKind::Main,
                variants: vec![],
            },
        )]);
        let products = [ProductRecord {
            product_id: 77,
            group_id: 1,
            name: "Senator Chuchi".into(),
            number: Some("1125".into()),
        }];
        let j = join(
            &[printing("u36", "36"), printing("u37", "37")],
            &products,
            &groups,
            &BTreeMap::new(),
        );
        assert!(j.links.is_empty(), "{:?}", j.links);
        let kinds: Vec<(&str, &str)> = j
            .alarms
            .iter()
            .map(|a| (a.kind.as_str(), a.key.as_str()))
            .collect();
        assert_eq!(kinds, vec![("ambiguous_tcgcsv", "77")]);

        // One printing alone still joins.
        let j = join(
            &[printing("u36", "36")],
            &products,
            &groups,
            &BTreeMap::new(),
        );
        assert_eq!(j.links.len(), 1);
        assert!(j.alarms.is_empty(), "{:?}", j.alarms);
    }
}
