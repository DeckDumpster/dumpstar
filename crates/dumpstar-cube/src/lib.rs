//! The rule checkers of the `swu-jumpstart-cube` skill: [`check_pack`] and [`check_cube`].
//!
//! They read the deck JSON the `/mcp` tools return and judge it against Ryan's cube rules
//! (`.claude/skills/swu-jumpstart-cube/SKILL.md`). The server holds none of these rules; this
//! crate is not linked into it.
//!
//! # Input
//!
//! A *pack document* is `get_deck`'s result as-is: it carries the card objects its lines name, as
//! `search_cards`/`get_card` return them, under `cards` keyed by card id:
//!
//! ```json
//! {"deck": {"name": "Pack 1", "group": "swu-jumpstart-cube"},
//!  "lines": [{"card_id": "SOR-5", "qty": 1, "zone": "main", "reserved": 1}],
//!  "cards": {"SOR-5": {"name": "...", "type": "Leader", "cost": 5,
//!                       "game": {"aspects": ["Vigilance", "Heroism"], "arenas": ["Ground"], "hp": 6}}}}
//! ```
//!
//! A *cube document* is an array of pack documents, or `{"packs": [...], "cards": {...}}` where
//! `cards` supplies any card a pack's own `cards` lacks.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde_json::Value;

pub const COLORS: [&str; 4] = ["Command", "Vigilance", "Aggression", "Cunning"];
pub const ALIGNMENTS: [&str; 2] = ["Heroism", "Villainy"];
/// Ryan's preferred 25-HP epic-action base of each color; a preference, not a requirement.
pub const EPIC_BASES: [(&str, &str); 4] = [
    ("Command", "Energy Conversion Lab"),
    ("Vigilance", "Security Complex"),
    ("Aggression", "Tarkintown"),
    ("Cunning", "Jedha City"),
];

/// Real 25-HP epic bases, recognised by name when the card object carries no `epic_action`.
pub const KNOWN_EPIC_BASES: [&str; 9] = [
    "Energy Conversion Lab",
    "Security Complex",
    "Tarkintown",
    "Jedha City",
    "Mystic Monastery",
    "Vergence Temple",
    "Shadow Collective Camp",
    "Temple of Destruction",
    "Tomb of Eilram",
];

pub const PACK_SIZE: u32 = 18;
pub const DRAW_CARDS: u32 = 15;
pub const CUBE_PACKS: usize = 20;
pub const PACKS_PER_COLOR: usize = 5;
pub const GROUND_UNITS: u32 = 8;
/// Cost 1, 2, 3, 4, 5, 6+: the inclusive quantity range of each.
pub const CURVE: [(u32, u32); 6] = [(0, 1), (4, 5), (4, 5), (2, 3), (1, 2), (1, 1)];

/// One broken rule: a short `rule` slug, and what is wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub rule: &'static str,
    pub message: String,
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.rule, self.message)
    }
}

fn fail(rule: &'static str, message: impl Into<String>) -> Failure {
    Failure {
        rule,
        message: message.into(),
    }
}

/// A card the checkers care about, read from a `search_cards`/`get_card` card object.
#[derive(Debug, Clone)]
struct Card {
    name: String,
    kind: String,
    cost: Option<u32>,
    hp: Option<i64>,
    /// `Some` when the card object carries a non-null `epic_action`; a null is "not said".
    epic_action: Option<bool>,
    aspects: BTreeSet<String>,
    arenas: BTreeSet<String>,
}

impl Card {
    fn from_value(v: &Value) -> Card {
        let strings = |p: &Value| -> BTreeSet<String> {
            p.as_array()
                .into_iter()
                .flatten()
                .filter_map(|s| s.as_str().map(String::from))
                .collect()
        };
        let game = &v["game"];
        Card {
            name: v["name"].as_str().unwrap_or("?").to_string(),
            kind: v["type"].as_str().unwrap_or("").to_string(),
            cost: v["cost"].as_u64().and_then(|c| u32::try_from(c).ok()),
            hp: game["hp"].as_i64(),
            epic_action: game
                .get("epic_action")
                .filter(|e| !e.is_null())
                .map(|e| e.as_str().is_some_and(|t| !t.is_empty())),
            aspects: strings(&game["aspects"]),
            arenas: strings(&game["arenas"]),
        }
    }

    /// Whether this base has an epic action: the card object's word, else the known names.
    fn is_epic(&self) -> bool {
        self.epic_action
            .unwrap_or_else(|| KNOWN_EPIC_BASES.contains(&self.name.as_str()))
    }

    fn colors(&self) -> Vec<&str> {
        COLORS
            .into_iter()
            .filter(|c| self.aspects.contains(*c))
            .collect()
    }

    fn alignments(&self) -> Vec<&str> {
        ALIGNMENTS
            .into_iter()
            .filter(|a| self.aspects.contains(*a))
            .collect()
    }
}

struct Line {
    card_id: String,
    qty: u32,
    sideboard: bool,
    reserved: Option<u32>,
}

/// A pack with its cards resolved.
pub struct Pack {
    name: String,
    lines: Vec<Line>,
    cards: BTreeMap<String, Card>,
}

impl Pack {
    /// Reads a pack document; `fallback` supplies cards the pack's own `cards` lacks.
    pub fn from_value(v: &Value, fallback: &Value) -> Result<Pack, String> {
        let lines = v
            .get("lines")
            .and_then(Value::as_array)
            .ok_or("a pack document needs a 'lines' array (get_deck's result)")?;
        let mut cards = BTreeMap::new();
        for source in [fallback, &v["cards"]] {
            if let Some(m) = source.as_object() {
                for (id, c) in m {
                    cards.insert(id.clone(), Card::from_value(c));
                }
            }
        }
        Ok(Pack {
            name: v["deck"]["name"]
                .as_str()
                .unwrap_or("(unnamed)")
                .to_string(),
            lines: lines
                .iter()
                .map(|l| Line {
                    card_id: l["card_id"].as_str().unwrap_or("").to_string(),
                    qty: l["qty"]
                        .as_u64()
                        .and_then(|q| u32::try_from(q).ok())
                        .unwrap_or(0),
                    sideboard: l["zone"].as_str() == Some("sideboard"),
                    reserved: l["reserved"].as_u64().and_then(|q| u32::try_from(q).ok()),
                })
                .collect(),
            cards,
        })
    }

    /// The leader's alignment and color, when the pack has exactly one valid leader.
    fn leader(&self) -> Option<(String, String)> {
        let leaders: Vec<&Card> = self.of_kind("Leader").map(|(c, _)| c).collect();
        let [l] = leaders.as_slice() else { return None };
        match (l.alignments().as_slice(), l.colors().as_slice()) {
            ([a], [c]) => Some((a.to_string(), c.to_string())),
            _ => None,
        }
    }

    fn of_kind<'a>(&'a self, kind: &'a str) -> impl Iterator<Item = (&'a Card, u32)> + 'a {
        self.lines.iter().filter_map(move |l| {
            self.cards
                .get(&l.card_id)
                .filter(|c| c.kind == kind)
                .map(|c| (c, l.qty))
        })
    }
}

/// The hard checks of one pack. An empty result passes.
pub fn check_pack(pack: &Pack) -> Vec<Failure> {
    let mut out = Vec::new();

    for l in &pack.lines {
        if !pack.cards.contains_key(&l.card_id) {
            out.push(fail(
                "unknown-card",
                format!("card '{}' has no card object to check against", l.card_id),
            ));
        }
        if l.sideboard {
            out.push(fail(
                "shape",
                format!("'{}' is in the sideboard; a pack has none", l.card_id),
            ));
        }
        if let Some(r) = l.reserved
            && r < l.qty
        {
            out.push(fail(
                "reserved",
                format!("'{}' is reserved {r} of {} copies", l.card_id, l.qty),
            ));
        }
    }

    let total: u32 = pack.lines.iter().map(|l| l.qty).sum();
    if total != PACK_SIZE {
        out.push(fail("shape", format!("{total} cards, not {PACK_SIZE}")));
    }
    let count = |kind: &str| pack.of_kind(kind).map(|(_, q)| q).sum::<u32>();
    if count("Leader") != 1 {
        out.push(fail("shape", format!("{} leaders, not 1", count("Leader"))));
    }
    if count("Base") != 2 {
        out.push(fail("shape", format!("{} bases, not 2", count("Base"))));
    }
    let draw: Vec<(&Card, u32)> = ["Unit", "Event", "Upgrade"]
        .into_iter()
        .flat_map(|k| pack.of_kind(k))
        .collect();
    let draw_count: u32 = draw.iter().map(|(_, q)| q).sum();
    if draw_count != DRAW_CARDS {
        out.push(fail(
            "shape",
            format!("{draw_count} draw cards, not {DRAW_CARDS}"),
        ));
    }
    for l in &pack.lines {
        if let Some(c) = pack.cards.get(&l.card_id)
            && !["Leader", "Base", "Unit", "Event", "Upgrade"].contains(&c.kind.as_str())
        {
            out.push(fail(
                "shape",
                format!("'{}' is a {}, not a draw card", c.name, c.kind),
            ));
        }
    }

    let leader = pack.of_kind("Leader").next().map(|(c, _)| c);
    if let Some(l) = leader
        && pack.leader().is_none()
    {
        out.push(fail(
            "leader",
            format!(
                "leader '{}' must have exactly one alignment and one color, has {:?}",
                l.name, l.aspects
            ),
        ));
    }

    let bases: Vec<&Card> = pack.of_kind("Base").map(|(c, _)| c).collect();
    let mut hps: Vec<i64> = bases.iter().filter_map(|b| b.hp).collect();
    hps.sort_unstable();
    if bases.len() == 2 && hps != [25, 30] {
        out.push(fail(
            "base-hp",
            format!("bases have HP {hps:?}, not one 25 and one 30"),
        ));
    }

    if let Some((alignment, color)) = pack.leader() {
        for b in &bases {
            if b.colors() != [color.as_str()] {
                out.push(fail(
                    "base-aspect",
                    format!(
                        "base '{}' is {:?}, the leader's color is {color}",
                        b.name,
                        b.colors()
                    ),
                ));
            }
            if b.hp == Some(25) && !b.is_epic() {
                out.push(fail(
                    "base-name",
                    format!(
                        "the 25-HP {color} base must be an epic base, but '{}' has no epic action",
                        b.name
                    ),
                ));
            }
            if b.hp == Some(30) && b.epic_action == Some(true) {
                out.push(fail(
                    "base-name",
                    format!(
                        "the 30-HP {color} base must be vanilla, but '{}' has an epic action",
                        b.name
                    ),
                ));
            }
        }
        for (c, _) in &draw {
            let off_color = c.colors().iter().any(|x| *x != color);
            let off_align = c.alignments().iter().any(|x| *x != alignment);
            if off_color || off_align {
                out.push(fail(
                    "draw-aspect",
                    format!(
                        "'{}' has aspects {:?}, the leader is {alignment} {color}",
                        c.name, c.aspects
                    ),
                ));
            }
        }
    }

    let (mut ground, mut space, mut spells) = (0, 0, 0);
    for (c, q) in &draw {
        match c.kind.as_str() {
            "Unit" if c.arenas.contains("Space") => space += q,
            "Unit" => ground += q,
            _ => spells += q,
        }
    }
    if draw_count == DRAW_CARDS {
        if ground != GROUND_UNITS {
            out.push(fail(
                "arena",
                format!("{ground} ground units, not {GROUND_UNITS}"),
            ));
        }
        if !(3..=4).contains(&space) {
            out.push(fail("arena", format!("{space} space units, not 3 or 4")));
        }
        if !(3..=4).contains(&spells) {
            out.push(fail(
                "arena",
                format!("{spells} events and upgrades, not 3 or 4"),
            ));
        }
    }

    let mut curve = [0u32; 6];
    for (c, q) in &draw {
        match c.cost {
            Some(cost) => curve[(cost.clamp(1, 6) - 1) as usize] += q,
            None => out.push(fail("curve", format!("'{}' has no cost", c.name))),
        }
    }
    for (i, (lo, hi)) in CURVE.iter().enumerate() {
        let label = if i == 5 {
            "6+".to_string()
        } else {
            (i + 1).to_string()
        };
        if !(*lo..=*hi).contains(&curve[i]) {
            out.push(fail(
                "curve",
                format!("{} cards at cost {label}, want {lo} to {hi}", curve[i]),
            ));
        }
    }
    out
}

/// Reads a pack document and checks it.
pub fn check_pack_value(doc: &Value) -> Result<Vec<Failure>, String> {
    Ok(check_pack(&Pack::from_value(doc, &Value::Null)?))
}

/// The pack checks of every pack, then the cube's: 20 packs, 10 of each alignment, 5 of each
/// color's base pair.
pub fn check_cube(packs: &[Pack]) -> Vec<Failure> {
    let mut out = Vec::new();
    for p in packs {
        out.extend(check_pack(p).into_iter().map(|f| Failure {
            rule: f.rule,
            message: format!("pack '{}': {}", p.name, f.message),
        }));
    }
    if packs.len() != CUBE_PACKS {
        out.push(fail(
            "cube-size",
            format!("{} packs, not {CUBE_PACKS}", packs.len()),
        ));
    }
    let mut alignments: BTreeMap<String, usize> = BTreeMap::new();
    let mut colors: BTreeMap<String, usize> = BTreeMap::new();
    for (a, c) in packs.iter().filter_map(Pack::leader) {
        *alignments.entry(a).or_default() += 1;
        *colors.entry(c).or_default() += 1;
    }
    for a in ALIGNMENTS {
        let n = alignments.get(a).copied().unwrap_or(0);
        if n != CUBE_PACKS / 2 {
            out.push(fail(
                "cube-alignment",
                format!("{n} {a} packs, not {}", CUBE_PACKS / 2),
            ));
        }
    }
    for c in COLORS {
        let n = colors.get(c).copied().unwrap_or(0);
        if n != PACKS_PER_COLOR {
            out.push(fail(
                "cube-bases",
                format!("{n} {c} base pairs, not {PACKS_PER_COLOR}"),
            ));
        }
    }
    out
}

/// Reads a cube document (see the crate docs) and checks it.
pub fn check_cube_value(doc: &Value) -> Result<Vec<Failure>, String> {
    let (list, shared) = match doc {
        Value::Array(a) => (a, &Value::Null),
        Value::Object(_) => (
            doc["packs"]
                .as_array()
                .ok_or("a cube document is an array of packs or {\"packs\": [...]}")?,
            &doc["cards"],
        ),
        _ => return Err("a cube document is an array of packs or {\"packs\": [...]}".into()),
    };
    let packs = list
        .iter()
        .map(|p| Pack::from_value(p, shared))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(check_cube(&packs))
}

/// Builders for the passing cube and its broken variants, shared by this crate's tests and the
/// scripted run in `dumpstar-server`.
pub mod fixture {
    use serde_json::{Value, json};

    use super::{EPIC_BASES, Pack};

    /// The 15-card skeleton of a pack: `(type, arena, cost)`. 8 ground, 3 space, 4 spells; the
    /// curve is 1, 4, 4, 3, 2, 1.
    pub const SKELETON: [(&str, &str, u32); 15] = [
        ("Unit", "Ground", 2),
        ("Unit", "Ground", 2),
        ("Unit", "Ground", 3),
        ("Unit", "Ground", 3),
        ("Unit", "Ground", 4),
        ("Unit", "Ground", 4),
        ("Unit", "Ground", 5),
        ("Unit", "Ground", 6),
        ("Unit", "Space", 2),
        ("Unit", "Space", 2),
        ("Unit", "Space", 3),
        ("Event", "", 1),
        ("Event", "", 3),
        ("Upgrade", "", 4),
        ("Event", "", 5),
    ];

    pub fn card(
        name: &str,
        kind: &str,
        cost: u32,
        arena: &str,
        hp: Option<i64>,
        aspects: &[&str],
    ) -> Value {
        json!({
            "name": name, "type": kind, "cost": cost,
            "game": {"aspects": aspects, "arenas": if arena.is_empty() { vec![] } else { vec![arena] }, "hp": hp},
        })
    }

    /// A passing pack document of the given alignment and color; `n` keeps card ids distinct.
    pub fn pack(n: usize, alignment: &str, color: &str) -> Value {
        let epic = EPIC_BASES.iter().find(|(c, _)| *c == color).unwrap().1;
        let mut cards = serde_json::Map::new();
        let mut lines = vec![];
        let mut put = |id: String, c: Value| {
            lines.push(json!({"card_id": id, "qty": 1, "zone": "main", "reserved": 1}));
            cards.insert(id, c);
        };
        put(
            format!("L{n}"),
            card(
                &format!("Leader {n}"),
                "Leader",
                5,
                "Ground",
                Some(6),
                &[alignment, color],
            ),
        );
        put(
            format!("B25-{n}"),
            card(epic, "Base", 0, "", Some(25), &[color]),
        );
        put(
            format!("B30-{n}"),
            card(
                &format!("{color} Vanilla Base"),
                "Base",
                0,
                "",
                Some(30),
                &[color],
            ),
        );
        for (i, (kind, arena, cost)) in SKELETON.iter().enumerate() {
            put(
                format!("C{n}-{i}"),
                card(
                    &format!("Card {n}-{i}"),
                    kind,
                    *cost,
                    arena,
                    None,
                    &[alignment, color],
                ),
            );
        }
        json!({"deck": {"name": format!("Pack {n}"), "group": "swu-jumpstart-cube"}, "lines": lines, "cards": cards})
    }

    /// The passing cube: 20 packs, 10 of each alignment, 5 per color.
    pub fn cube() -> Vec<Value> {
        let split = [
            ("Command", 3),
            ("Vigilance", 2),
            ("Aggression", 3),
            ("Cunning", 2),
        ];
        let mut packs = vec![];
        for (color, heroes) in split {
            for k in 0..5 {
                let a = if k < heroes { "Heroism" } else { "Villainy" };
                packs.push(pack(packs.len() + 1, a, color));
            }
        }
        packs
    }

    pub fn parsed(v: &Value) -> Pack {
        Pack::from_value(v, &Value::Null).unwrap()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::fixture::{cube, pack};
    use super::*;

    fn rules(doc: &Value) -> Vec<&'static str> {
        check_pack_value(doc)
            .unwrap()
            .into_iter()
            .map(|f| f.rule)
            .collect()
    }

    /// Replaces `f(card)` in the line whose card id starts with `id`.
    fn edit(doc: &mut Value, id: &str, f: impl Fn(&mut Value)) {
        let key = doc["cards"]
            .as_object()
            .unwrap()
            .keys()
            .find(|k| k.starts_with(id))
            .unwrap()
            .clone();
        f(&mut doc["cards"][key]);
    }

    fn remove_line(doc: &mut Value, id: &str) {
        let lines = doc["lines"].as_array_mut().unwrap();
        lines.retain(|l| !l["card_id"].as_str().unwrap().starts_with(id));
    }

    #[test]
    fn a_well_formed_pack_passes() {
        assert_eq!(
            check_pack_value(&pack(1, "Heroism", "Vigilance")).unwrap(),
            vec![]
        );
    }

    #[test]
    fn pack_shape_fails_on_a_missing_card_and_a_second_leader() {
        let mut p = pack(1, "Heroism", "Command");
        remove_line(&mut p, "C1-14");
        assert!(rules(&p).contains(&"shape"));
        let mut p = pack(1, "Heroism", "Command");
        edit(&mut p, "C1-0", |c| c["type"] = json!("Leader"));
        assert!(rules(&p).contains(&"shape"));
    }

    #[test]
    fn a_sideboard_line_fails_the_shape() {
        let mut p = pack(1, "Villainy", "Cunning");
        p["lines"][3]["zone"] = json!("sideboard");
        assert!(rules(&p).contains(&"shape"));
    }

    #[test]
    fn a_leader_needs_one_alignment_and_one_color() {
        let mut p = pack(1, "Heroism", "Command");
        edit(&mut p, "L1", |c| {
            c["game"]["aspects"] = json!(["Heroism", "Command", "Cunning"])
        });
        assert!(rules(&p).contains(&"leader"));
    }

    #[test]
    fn bases_must_be_25_and_30_hp() {
        let mut p = pack(1, "Heroism", "Command");
        edit(&mut p, "B30", |c| c["game"]["hp"] = json!(28));
        assert!(rules(&p).contains(&"base-hp"));
    }

    #[test]
    fn a_base_must_match_the_leaders_color() {
        let mut p = pack(1, "Heroism", "Command");
        edit(&mut p, "B30", |c| c["game"]["aspects"] = json!(["Cunning"]));
        assert!(rules(&p).contains(&"base-aspect"));
    }

    #[test]
    fn the_25_hp_base_must_be_an_epic_base() {
        let mut p = pack(1, "Heroism", "Command");
        edit(&mut p, "B25", |c| c["name"] = json!("Plain Outpost"));
        assert!(rules(&p).contains(&"base-name"));
        let mut p = pack(1, "Heroism", "Command");
        edit(&mut p, "B25", |c| c["game"]["epic_action"] = json!(null));
        assert!(rules(&p).contains(&"base-name"));
    }

    #[test]
    fn a_null_epic_action_on_a_known_epic_base_falls_back_to_the_name() {
        let mut p = pack(1, "Heroism", "Command");
        edit(&mut p, "B25", |c| {
            c["name"] = json!("Security Complex");
            c["game"]["epic_action"] = json!(null);
        });
        assert!(rules(&p).is_empty());
    }

    #[test]
    fn any_real_25_hp_epic_base_of_the_color_is_accepted() {
        let mut p = pack(1, "Heroism", "Command");
        edit(&mut p, "B25", |c| c["name"] = json!("Mystic Monastery"));
        assert!(rules(&p).is_empty());
        let mut p = pack(1, "Heroism", "Command");
        edit(&mut p, "B25", |c| {
            c["name"] = json!("Some Future Base");
            c["game"]["epic_action"] = json!("Epic Action: Draw a card.");
        });
        assert!(rules(&p).is_empty());
    }

    #[test]
    fn any_real_30_hp_vanilla_base_is_accepted() {
        let mut p = pack(1, "Heroism", "Command");
        edit(&mut p, "B30", |c| {
            c["name"] = json!("Echo Base");
            c["game"]["epic_action"] = json!(null);
        });
        assert!(rules(&p).is_empty());
        edit(&mut p, "B30", |c| {
            c["game"]["epic_action"] = json!("Epic Action: Draw a card.")
        });
        assert!(rules(&p).contains(&"base-name"));
    }

    #[test]
    fn the_base_name_message_names_the_actual_base() {
        let mut p = pack(1, "Heroism", "Command");
        edit(&mut p, "B25", |c| c["name"] = json!("Plain Outpost"));
        let f = check_pack_value(&p).unwrap();
        assert_eq!(
            f[0].message,
            "the 25-HP Command base must be an epic base, but 'Plain Outpost' has no epic action"
        );
    }

    #[test]
    fn a_draw_card_must_share_the_leaders_alignment_and_color() {
        let mut p = pack(1, "Heroism", "Command");
        edit(&mut p, "C1-3", |c| {
            c["game"]["aspects"] = json!(["Villainy", "Command"])
        });
        assert!(rules(&p).contains(&"draw-aspect"));
        let mut p = pack(1, "Heroism", "Command");
        edit(&mut p, "C1-3", |c| {
            c["game"]["aspects"] = json!(["Heroism", "Cunning"])
        });
        assert!(rules(&p).contains(&"draw-aspect"));
        let mut p = pack(1, "Heroism", "Command");
        edit(&mut p, "C1-3", |c| c["game"]["aspects"] = json!([]));
        assert!(rules(&p).is_empty(), "a neutral card is on every aspect");
    }

    #[test]
    fn the_arena_split_is_8_ground_3_or_4_space_3_or_4_spells() {
        let mut p = pack(1, "Heroism", "Command");
        edit(&mut p, "C1-8", |c| c["game"]["arenas"] = json!(["Ground"]));
        assert!(rules(&p).contains(&"arena"));
        let mut p = pack(1, "Heroism", "Command");
        for id in ["C1-11", "C1-12"] {
            edit(&mut p, id, |c| {
                c["type"] = json!("Unit");
                c["game"]["arenas"] = json!(["Space"]);
            });
        }
        assert!(rules(&p).contains(&"arena"));
    }

    #[test]
    fn the_curve_holds_each_cost_to_its_range() {
        let mut p = pack(1, "Heroism", "Command");
        edit(&mut p, "C1-0", |c| c["cost"] = json!(1));
        edit(&mut p, "C1-1", |c| c["cost"] = json!(1));
        assert!(rules(&p).contains(&"curve"));
        let mut p = pack(1, "Heroism", "Command");
        edit(&mut p, "C1-7", |c| c["cost"] = json!(4));
        assert!(rules(&p).contains(&"curve"), "no six-drop");
    }

    #[test]
    fn a_copy_not_reserved_fails() {
        let mut p = pack(1, "Heroism", "Command");
        p["lines"][4]["reserved"] = json!(0);
        assert!(rules(&p).contains(&"reserved"));
    }

    #[test]
    fn a_line_with_no_card_object_fails() {
        let mut p = pack(1, "Heroism", "Command");
        p["cards"].as_object_mut().unwrap().remove("L1");
        assert!(rules(&p).contains(&"unknown-card"));
    }

    fn cube_rules(packs: Vec<Value>) -> Vec<&'static str> {
        check_cube_value(&Value::Array(packs))
            .unwrap()
            .into_iter()
            .map(|f| f.rule)
            .collect()
    }

    #[test]
    fn a_well_formed_cube_passes() {
        assert_eq!(cube_rules(cube()), Vec::<&str>::new());
    }

    #[test]
    fn a_cube_of_19_packs_fails() {
        let mut c = cube();
        c.pop();
        assert!(cube_rules(c).contains(&"cube-size"));
    }

    #[test]
    fn a_cube_needs_ten_of_each_alignment() {
        let mut c = cube();
        c[0] = pack(1, "Villainy", "Command");
        assert!(cube_rules(c).contains(&"cube-alignment"));
    }

    #[test]
    fn a_cube_needs_five_of_each_base_pair() {
        let mut c = cube();
        c[0] = pack(1, "Heroism", "Cunning");
        assert!(cube_rules(c).contains(&"cube-bases"));
    }

    #[test]
    fn a_broken_pack_inside_a_cube_is_named() {
        let mut c = cube();
        edit(&mut c[4], "C5-3", |x| x["cost"] = json!(1));
        let f = check_cube_value(&Value::Array(c)).unwrap();
        assert!(f.iter().any(|f| f.message.contains("pack 'Pack 5'")));
    }

    #[test]
    fn a_cube_document_may_share_its_cards() {
        let mut packs = cube();
        let mut shared = serde_json::Map::new();
        for p in &mut packs {
            for (k, v) in p["cards"].as_object().unwrap().clone() {
                shared.insert(k, v);
            }
            p.as_object_mut().unwrap().remove("cards");
        }
        let doc = json!({"packs": packs, "cards": shared});
        assert_eq!(check_cube_value(&doc).unwrap(), vec![]);
    }
}
