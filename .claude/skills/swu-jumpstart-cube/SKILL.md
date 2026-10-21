---
name: swu-jumpstart-cube
description: Build a 20-pack Star Wars: Unlimited Jumpstart cube from the user's own collection through the dumpstar /mcp tools, saving each pack as a user-defined deck and checking it with swu-cube-check. Use when asked to build, extend or audit a SWU jumpstart cube.
---

# SWU Jumpstart cube

All the judgement is here. The dumpstar server enforces none of these rules: a pack is a
user-defined deck to it (free-text format label, a group), and it checks only that you own the
copies and that no copy is in two decks. `swu-cube-check` (below) is what tells you the rules hold.

## The cube rules (Ryan's document)

### 1. Format

- **Cube:** 20 half-decks, 10 Heroism and 10 Villainy.
- **Pack:** 18 cards.
- **Singleton Leaders:** across the 20 packs each Leader leads at most one pack (by card, so two
  printings count as one). Bases and draw cards may repeat across packs; the collection's copies
  are the only limit, and the deck reservations already enforce that. `swu-cube-check cube` fails
  `cube-singleton`, naming the Leader and its packs; `pack` mode does not.
- **Match:** two random packs make one 30-card draw deck.

### 2. The 18-card pack

1. **1 Leader.** It must be Heroism or Villainy: a **Neutral** leader (no alignment, such as
   Fennec Shand) does not lead a pack, and `swu-cube-check` rejects one. A leader with an alignment but **no color aspect** (Anakin Skywalker, Tempted by the Dark Side, LOF-18: the official record lists Heroism only) cannot lead a pack either, since a pack is defined by its leader's color; the checker rejects it with that message, so pick another leader. It sets the alignment (Heroism/Villainy, the aspect names; `game.alignment` and the summary use them too) and the color aspect.
2. **1 Epic Action Base (25 HP)**, strictly the Leader's color aspect: any 25-HP base of that aspect with an epic action.
3. **1 Vanilla Base (30 HP)**, strictly the Leader's color aspect: any 30-HP base of that aspect without an epic action.
4. **15 draw cards**, none off the Leader's alignment or color aspect (no off-aspect penalties).
   A card of the Leader's color with no Heroism/Villainy aspect (Make an Opening, Vanquish) carries
   no aspect penalty and is allowed; so is a card with no aspect at all. A card of another color,
   or of the other alignment, is not.

### 3. Pre-game setup (why the aspects must match)

A player combines Pack A and Pack B: the Leader comes from A (alignment), the Base from B (color
aspect, which removes the 3-aspect penalty), and B's two bases give the choice of tempo (25 HP
epic action) or health (30 HP vanilla).

### 4. The 15-card skeleton

**Arena and type split**

- 8 ground units.
- 3-4 space units (non-negotiable minimum, so the combined deck can contest space).
- 3-4 events or upgrades (unconditional removal, direct damage, stat buffs).

**Resource curve**

| Cost | Quantity | Purpose |
| --- | --- | --- |
| 1 | 0 to 1 | Optional. Premium upgrades or events only. |
| 2 | 4 to 5 | Critical. Ensures a turn-1 play. |
| 3 | 4 to 5 | Critical. Maintains tempo on turn 2. |
| 4 | 2 to 3 | Mid-game stabilizers and Sentinels. |
| 5 | 1 to 2 | Heavy hitters before the Leader flips. |
| 6+ | 1 | The Bomb: one massive unit. |

### 5. Command zone

Exactly **5 packs per color** across the 20, each with an epic base and a vanilla base of that
color. There are no cards named "Vanilla <color> Base": the vanilla base is any 30-HP base of the
Leader's aspect without an epic action, and the epic base is any 25-HP base of that aspect with one
(`epic_action` on the card object). Ryan's preferred epic bases are the preference, not a requirement:

| Color | Preferred epic (25 HP) |
| --- | --- |
| Command (Green) | Energy Conversion Lab |
| Vigilance (Blue) | Security Complex |
| Aggression (Red) | Tarkintown |
| Cunning (Yellow) | Jedha City |

One rule decides: an epic base is a 25-HP base whose card object has a non-null `epic_action`. A
base is not epic by name; Mystic Monastery and similar bases whose `epic_action` is null are
rejected as the epic base. A collection may not own enough epic bases for 5 per
color (read the owned epic-base counts from `collection_summary` at run time): report the shortfall to the user and build the packs
that are possible; never fail silently.

**Leaders: the universality rule.** Prefer Common Leaders with universal, unconditional abilities
(damage, shielding, resource readying). Avoid Rare, parasitic Leaders whose abilities need
specific traits (Hera Syndulla, Grand Inquisitor) unless the other half-deck will support them.
Top-tier universal examples: Sabine Wren (SOR-14), Boba Fett (SOR-15), Darth Vader. Leia Organa, Alliance General (SOR-9) is not one: her ability needs Rebel units.

### 6. Archetype blueprints

For the 15 draw cards, favor universal "goodstuff" and additive mechanics (Shields, Sentinels,
Smuggle, Raid) that improve whatever pack they meet.

1. **Rebel Vanguard** (Heroism / Command): go-wide aggro with tribal buffs. 2-cost Rebels, General Dodonna, Medal Ceremony, You're In Command.
2. **Imperial Armor** (Villainy / Command): resource acceleration into heavy bombs. Superlaser Technician/Resupply, Overwhelming Barrage, AT-ST/Relentless.
3. **Defensive Fortress** (Any / Vigilance): hard control and attrition. Regional Governor/Sentinels, Protective Shields, Takedown/Vanquish.
4. **Underworld Smugglers** (Any / Cunning): board manipulation, tempo exhaust, Smuggle. Crafty Smuggler, Waylay, Asteroid Sanctuary, Shoot First.
5. **Aggressive Assault** (Any / Aggression): direct damage and raid. Sabine Wren/Disruptor Sniper, Open Fire, Keep Fighting.
6. **Starfighter Fleet** (Any / flex aspect): space skew. At least 6 space units (A-Wings/Interceptors), Wing Leader, Tactical Advantage.

The archetypes and the leader guidance are advice. `swu-cube-check` does not test them.

## Method

**Group name.** Ask the user which group the packs go in; default `swu-jumpstart-cube`. A partial
or test build names its own group so it never lands in the real cube's group. Use that one group
for every `create_deck` and `list_decks` below.

Mind the rate limit: `/mcp` allows about 120 tool calls a minute per sign-in. Use a fixed handful
of searches per pack (step 3) and choose from the results yourself; never search once per card.

1. **`collection_summary`.** Learn what is owned and what is still available. A cube needs about
   20 Leaders, 40 bases (5 epic and 5 vanilla per color) and 300 draw cards. Leaders are singleton,
   so count **distinct** Leaders per color; draw cards and bases may repeat, so copies count for them.
2. **Allocate leaders and base pairs.** Pick 20 Leaders: 10 Heroism, 10 Villainy, 5 of each color
   (for example Command 3+2, Vigilance 2+3, Aggression 3+2, Cunning 2+3). Apply the universality
   rule. Each pack takes an epic and a vanilla base of its Leader's color; if the collection lacks them, say so and build what is possible.
3. **Plan copies across the whole cube.** Before building any pack, run `swu-cube-check allocate`.
   Heroism-only, Villainy-only and no-aspect cards (Longbeam Cruiser) fit packs of any color, so an
   early pack's picks can use up copies a later color depends on, and the per-pack checker cannot
   see it. Run the four searches below for every pack first, then write
   `{"owned": {"<card id>": <available copies>}, "packs": [{"name": "Pack 1", "slots": 15,
   "eligible": ["<card id>", ...]}]}` (eligible = the card ids that pack's searches returned) and pipe it to
   `swu-cube-check allocate`. It reserves each card's copies to specific packs (`reserve <card>: Pack 2`)
   and prints `ok`, or `FAIL allocation:` naming the card and the packs competing for it, which you
   report to the user. Build each pack only from the cards reserved to it; a pack takes one copy of a card.
4. **Fill each pack to the skeleton.** Four `search_cards` per pack cover the Leader's whole pool,
   because section 2 allows four sets and no one query returns them all: color and alignment, color
   only, alignment only, and no aspect at all. None returns Leaders or bases, which are chosen
   separately. All pass `avail>=1` in the query (when `query_help` lists `avail`; otherwise
   pass `owned_only: true` and keep results whose `available_count` is at least 1), `limit: 100`.
   `<other colors>` is each color but the Leader's (`-a:<other color>` once per color), and `<other
   alignment>` is the opposite of the Leader's:
   - color+alignment: `a:<color> a:<alignment> -a:<each other color> -t:leader -t:base avail>=1`
   - color only: `a:<color> -a:heroism -a:villainy -a:<each other color> -t:leader -t:base avail>=1`
   - alignment only: `a:<alignment> -a:<every color> -t:leader -t:base avail>=1`
   - no aspect: `-a:heroism -a:villainy -a:<every color> -t:leader -t:base avail>=1`

   `<every color>` is all four colors (`-a:command -a:vigilance -a:aggression -a:cunning`). Each
   `-a:<other color>` drops the cards that carry a second color (a Vigilance + Cunning unit, a
   three-aspect IG-88): they fail the pack's aspect rule. For Vigilance and Heroism the color+alignment
   query is `a:vigilance a:heroism -a:command -a:aggression -a:cunning -t:leader -t:base avail>=1`.
   The alignment-only set is cards such as Alliance X-Wing and Wing Leader (Heroism, no color); a
   Villainy leader's pool has AT-ST there. The checker accepts exactly these four sets.

   One more for the color's bases: `t:base a:<color>`. Then pick from the combined results the 15
   cards that meet section 4, the 25-HP and 30-HP bases, and the Leader. A card reserved to another pack in step 3 is not yours to pick. Prefer
   cards of the Leader's own alignment and color; same-color cards with no alignment, cards of the Leader's
   alignment with no color, and cards with no aspects are also allowed.
5. **Check the planned pack, then save it.** Before any write, assemble the pack document (below)
   and run `swu-cube-check pack --planned`. Fix every `FAIL` line, then `create_deck` (`format`:
   `SWU Jumpstart Cube`, `group`: the group name (the same for all 20 packs), `name`: `Pack <n>`) and
   `set_deck_lines` with the 18 `{card_id, qty: 1}` lines. A refusal says which lines are short and
   changes nothing: swap those cards and retry.
6. **Check the whole group.** `list_decks` with that group, `get_deck` for each of the 20, save
   each result to its own file and run `swu-cube-check cube pack1.json pack2.json ...` (no combine
   script needed). `reserved` must equal `qty` on every line. The cube-level check applies to a
   complete 20-pack cube only; a partial or test build checks each saved deck with
   `swu-cube-check pack <file>` instead (`cube-size`, `cube-alignment` and `cube-bases` would fail).
   `get_deck` carries a `lines_digest` and the checker recomputes it from the file's `lines`. If it
   refuses with "this is not the saved deck: digest mismatch", your copy drifted: copy the
   `get_deck` result again. Never edit the file (or the digest) to make it pass.

## The checker

`swu-cube-check pack [--planned] [FILE]` and `swu-cube-check cube [--planned] [FILE...]` (FILE or stdin; `cube` also takes one pack file per deck) print `ok`, or one
`FAIL <rule>: <what is wrong>` line per broken rule, and exit 1 on any failure. Build it with
`cargo build -p dumpstar-cube --release` (binary `target/release/swu-cube-check`).

A **pack document** is `get_deck`'s result as-is: it carries the card object of every line under
`cards`, keyed by card id (the `<SET>-<number>` form). Each card has the shape of a `search_cards` row, or
of the `card` member of `get_card`'s result (`get_card` returns `{card, copies, printings}`; use
only `.card`). To check a pack before saving it, build the same document by hand from those:

```json
{"deck": {"name": "Pack 1", "group": "swu-jumpstart-cube"},
 "lines": [{"card_id": "SOR-5", "qty": 1, "zone": "main"}],
 "cards": {"SOR-5": {"name": "...", "type": "Leader", "cost": 5,
                        "game": {"aspects": ["Vigilance", "Heroism"], "arenas": ["Ground"], "hp": 6}}}}
```

The `reserved` rule is meaningful only on the post-save `get_deck` document, where the server
reports reservations. A planned (pre-save) document has none, so check it with `--planned`, which
ignores `reserved`; never fill `reserved` in by hand.

A **cube document** is an array of pack documents, or `{"packs": [...], "cards": {...}}` with the
cards shared. Rules reported: `shape`, `leader`, `base-hp`, `base-aspect`, `base-name`,
`draw-aspect`, `arena`, `curve`, `reserved`, `unknown-card`, and for the cube `cube-size`,
`cube-alignment`, `cube-bases`, `cube-singleton`. `draw-aspect` rejects a draw card with any color other than the Leader's or any
alignment other than the Leader's. A card with no alignment (of the Leader's color) or with no
aspects at all passes.

Finish by telling the user which packs were built, which Leaders you chose and why, and the
`swu-cube-check cube` output.
