//! `swu-cube-check allocate`: plan copy allocation across the whole cube before any pack is built.
//!
//! Heroism-only, Villainy-only and no-aspect cards fit packs of any color, so one pack's picks can
//! use up copies another color's packs depend on. The pack checker sees one pack at a time; this
//! assigns every pack's slots at once (max-flow), so a plan either reserves each copy to a pack or
//! names the card and the packs competing for it.
//!
//! Input: `{"owned": {"<card id>": copies}, "packs": [{"name": "Pack 1", "slots": 15,
//! "eligible": ["<card id>", ...]}]}`. A pack takes at most one copy of a card.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde_json::Value;

use crate::{Failure, fail};

pub struct Plan {
    /// card id -> the packs it is reserved to, in pack order.
    pub reservations: BTreeMap<String, Vec<String>>,
    pub failures: Vec<Failure>,
}

pub fn allocate_value(doc: &Value) -> Result<Plan, String> {
    let owned = doc
        .get("owned")
        .and_then(Value::as_object)
        .ok_or("allocate: `owned` must be an object of card id -> copies")?;
    let packs = doc
        .get("packs")
        .and_then(Value::as_array)
        .ok_or("allocate: `packs` must be an array")?;
    let mut copies: BTreeMap<&str, usize> = BTreeMap::new();
    for (id, n) in owned {
        let n = n
            .as_u64()
            .ok_or(format!("allocate: copies of {id} is not a number"))?;
        copies.insert(id, n as usize);
    }
    let mut names = Vec::new();
    let mut slots = Vec::new();
    let mut eligible: Vec<Vec<&str>> = Vec::new();
    for (i, p) in packs.iter().enumerate() {
        names.push(
            p.get("name")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or(format!("Pack {}", i + 1)),
        );
        slots.push(
            p.get("slots")
                .and_then(Value::as_u64)
                .ok_or("allocate: a pack lacks `slots`")? as usize,
        );
        let e = p
            .get("eligible")
            .and_then(Value::as_array)
            .ok_or("allocate: a pack lacks `eligible`")?;
        let mut ids = BTreeSet::new();
        for c in e {
            ids.insert(
                c.as_str()
                    .ok_or("allocate: eligible entries are card ids")?,
            );
        }
        eligible.push(
            ids.into_iter()
                .filter(|c| copies.get(c).copied().unwrap_or(0) > 0)
                .collect(),
        );
    }

    // held[p] = cards assigned to pack p; users[c] = packs holding c.
    let mut held: Vec<BTreeSet<&str>> = vec![BTreeSet::new(); packs.len()];
    let mut users: BTreeMap<&str, BTreeSet<usize>> = BTreeMap::new();
    for p in 0..packs.len() {
        while held[p].len() < slots[p] {
            if !augment(p, &eligible, &copies, &mut held, &mut users) {
                break;
            }
        }
    }

    let mut failures = Vec::new();
    for p in 0..packs.len() {
        let short = slots[p] - held[p].len();
        if short == 0 {
            continue;
        }
        // Everything the stuck pack could displace: walk cards it wants and the packs holding them.
        let mut seen_packs = BTreeSet::from([p]);
        let mut contested: BTreeSet<&str> = BTreeSet::new();
        let mut queue = VecDeque::from([p]);
        while let Some(q) = queue.pop_front() {
            for &c in &eligible[q] {
                if held[q].contains(c) {
                    continue;
                }
                contested.insert(c);
                for &r in users.get(c).into_iter().flatten() {
                    if seen_packs.insert(r) {
                        queue.push_back(r);
                    }
                }
            }
        }
        let cards: Vec<String> = contested
            .iter()
            .map(|c| format!("{c} ({} owned)", copies[c]))
            .collect();
        let competing: Vec<&str> = seen_packs
            .iter()
            .filter(|&&q| q != p)
            .map(|&q| names[q].as_str())
            .collect();
        failures.push(fail(
            "allocation",
            format!(
                "{} is {short} card(s) short of {} even with every copy reassigned; the cards it can use are all spoken for: {}; competing packs: {}",
                names[p],
                slots[p],
                cards.join(", "),
                competing.join(", ")
            ),
        ));
    }
    let reservations = users
        .into_iter()
        .map(|(c, ps)| {
            (
                c.to_string(),
                ps.into_iter().map(|p| names[p].clone()).collect(),
            )
        })
        .collect();
    Ok(Plan {
        reservations,
        failures,
    })
}

/// Give pack `start` one more card, moving other packs' picks to alternatives where needed.
fn augment<'a>(
    start: usize,
    eligible: &[Vec<&'a str>],
    copies: &BTreeMap<&str, usize>,
    held: &mut [BTreeSet<&'a str>],
    users: &mut BTreeMap<&'a str, BTreeSet<usize>>,
) -> bool {
    // BFS over packs; edge p -> q when p wants a card (not held) that q holds and p cannot get free.
    let mut prev: BTreeMap<usize, (usize, &'a str)> = BTreeMap::new(); // pack -> (from pack, card taken from it)
    let mut seen = BTreeSet::from([start]);
    let mut queue = VecDeque::from([start]);
    while let Some(p) = queue.pop_front() {
        for &c in &eligible[p] {
            if held[p].contains(c) {
                continue;
            }
            let used = users.get(c).map_or(0, BTreeSet::len);
            if used < copies[c] {
                // Free copy: p takes it; each pack on the path back hands over what its
                // predecessor wanted and takes the next link in turn.
                let (mut cur, mut card) = (p, c);
                loop {
                    held[cur].insert(card);
                    users.entry(card).or_default().insert(cur);
                    match prev.get(&cur) {
                        Some(&(from, taken)) => {
                            held[cur].remove(taken);
                            users.get_mut(taken).unwrap().remove(&cur);
                            cur = from;
                            card = taken;
                        }
                        None => return true,
                    }
                }
            }
            for &q in users.get(c).into_iter().flatten() {
                if seen.insert(q) {
                    prev.insert(q, (p, c));
                    queue.push_back(q);
                }
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Pack-by-pack, first eligible card with a copy left: what the skill did before it planned.
    fn greedy(doc: &Value) -> Vec<String> {
        let mut left: BTreeMap<String, u64> = doc["owned"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.as_u64().unwrap()))
            .collect();
        let mut starved = Vec::new();
        for p in doc["packs"].as_array().unwrap() {
            let mut need = p["slots"].as_u64().unwrap();
            for c in p["eligible"].as_array().unwrap() {
                let n = left.get_mut(c.as_str().unwrap()).unwrap();
                if need > 0 && *n > 0 {
                    *n -= 1;
                    need -= 1;
                }
            }
            if need > 0 {
                starved.push(p["name"].as_str().unwrap().to_string());
            }
        }
        starved
    }

    /// Longbeam Cruiser has no aspect, so every pack may take it; the Vigilance pack has little else.
    fn starving() -> Value {
        json!({
            "owned": {"Longbeam Cruiser": 1, "Vigilance Unit": 1, "Command Unit": 1},
            "packs": [
                {"name": "Pack 1 (Command)", "slots": 1, "eligible": ["Longbeam Cruiser", "Command Unit"]},
                {"name": "Pack 2 (Vigilance)", "slots": 1, "eligible": ["Longbeam Cruiser"]},
            ]
        })
    }

    #[test]
    fn a_greedy_build_starves_the_last_pack_and_the_plan_does_not() {
        let doc = starving();
        assert_eq!(greedy(&doc), vec!["Pack 2 (Vigilance)"]);
        let plan = allocate_value(&doc).unwrap();
        assert!(plan.failures.is_empty(), "{:?}", plan.failures);
        assert_eq!(
            plan.reservations["Longbeam Cruiser"],
            vec!["Pack 2 (Vigilance)"]
        );
        assert_eq!(plan.reservations["Command Unit"], vec!["Pack 1 (Command)"]);
    }

    #[test]
    fn a_shortfall_names_the_card_and_the_competing_packs() {
        let mut doc = starving();
        doc["owned"]["Command Unit"] = json!(0);
        let plan = allocate_value(&doc).unwrap();
        assert_eq!(plan.failures.len(), 1);
        let m = &plan.failures[0].message;
        assert!(
            m.contains("Longbeam Cruiser (1 owned)") && m.contains("Pack 1 (Command)"),
            "{m}"
        );
    }

    #[test]
    fn a_pack_takes_one_copy_of_a_card() {
        let doc =
            json!({"owned": {"A": 2}, "packs": [{"name": "P", "slots": 2, "eligible": ["A"]}]});
        assert_eq!(allocate_value(&doc).unwrap().failures.len(), 1);
    }
}
