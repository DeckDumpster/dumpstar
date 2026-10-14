# SWU rules documents

Retrieved 2026-10-13 (UTC). Never edit these; re-retrieve instead. The format data in
`crates/dumpstar-catalog/data/formats.json` cites them and
`crates/dumpstar-core/tests/rules_citations.rs` checks every citation against the text.

| File | source_url | Shows |
|---|---|---|
| comprehensive-rules-v8.0.pdf (+.pdf.headers) | https://cdn.starwarsunlimited.com//SWH_Comp_Rules_v8_0_e26603c6e1.pdf | Comprehensive Rules v8.0, dated 7/8/26. sha256 18dd5c92b051b3260c535d3ef225abc5654ba65ec74093bbe3a50536db37c8fb. Verbatim. |
| comprehensive-rules-v8.0.extracted.txt.gz | derived from the PDF above | DERIVED, not verbatim: raw text-stream extraction (no PDF tool on the build box), aspect icons are lost (they are glyphs). Exists so tests can cite sections as text. |
| article-twin-suns.html.gz | https://starwarsunlimited.com/articles/twin-suns | official Twin Suns announcement |
| article-a-leader-suspended.html.gz | https://starwarsunlimited.com/articles/a-leader-suspended | Premier suspension of Boba Fett (Collecting the Bounty), 2024-11-08; suspension policy |
| article-suspension-update-april-2025.html.gz | https://starwarsunlimited.com/articles/suspension-update-april-2025 | Premier suspensions effective 2025-04-11 |
| article-eternal-format-update-april-2026.html.gz | https://starwarsunlimited.com/articles/eternal-format-update-april-2026 | Eternal suspensions are temporary; first Eternal suspensions effective 2026-04-24 |

## Not retrieved

The separate Tournament Regulations PDF: no stable link exists on starwarsunlimited.com
(the how-to-play and competitive-play pages link their documents from client-side
scripts) and a search returned none. The deckbuilding rules the design names are all in
the Comprehensive Rules (section 9.2 Premier & Eternal, 10 Limited, 12 Twin Suns), so the
format data rests on that. The current suspended-card list is likewise not captured:
it changes at each rotation and is published as articles, so `card_legality` is data
loaded by the catalog payload, not hard-coded here.
