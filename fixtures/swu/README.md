# SWU fixtures

Verbatim captures of the two data sources, retrieved 2026-10-12 by the Concierge's data-feasibility study (brain: wiki/projects/dumpstar/data-feasibility-2026-10-12.md). `manifest.md` lists each file's source URL and what it shows. Large JSON is gzipped; never edit these, re-retrieve instead.

- official-sor-pages/: all 10 pages of the official API for set SOR (991 printings), gzipped
- off-single-uid.json: single-card fetch by cardUid; off-cards-p2.json.gz: a bulk page
- swu-groups.json, sor-products.json.gz, sor-prices.json: tcgcsv category 79
- tcgcsv-swu-counts.txt: per-group product/price counts
- off-sec-shared-standard.json: **synthetic**, not a capture (sp-5zcm06). Two official entries shaped like the real SEC pair Senator Chuchi `06361125` / `06371125`: same set, number, variant name (Standard) and finish, different serial variant codes (36, 37) and cardUids. The real load had 21 such pairs, all SEC.
- tcgcsv-{set}-{group}-products.json.gz, tcgcsv-ash-24660-prices.json.gz, official-trim-*.json.gz (sp-9uiu2z): the six main sets JTL..HMW and the CST pair, for the TCGplayer-id coverage test; the official files are trimmed to the attributes the parser reads (see manifest.md)
