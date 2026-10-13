# raw/dumpstar manifest

Verbatim captures for the dumpstar (Star Wars: Unlimited) data-source feasibility study. Retrieved 2026-10-12 (US Pacific) with User-Agent `dumpstar-feasibility/0.1 (ryan@gantt.pub research probe)`, polite spacing (0.5-3 s). Probe scripts: `tools/dumpstar/`. Study: wiki/projects/dumpstar/data-feasibility-2026-10-12.md. Each `*.headers` file is the verbatim HTTP response header block for the file of the same stem.

| File | source_url | Shows |
|---|---|---|
| categories.json (+.headers) | https://tcgcsv.com/tcgplayer/categories | tcgcsv category list (54 KB); SWU = categoryId 79, Lorcana = 71 |
| swu-groups.json | https://tcgcsv.com/tcgplayer/79/groups | all 34 SWU groups (sets) with groupId, abbreviation, publishedOn |
| lorcana-groups.json | https://tcgcsv.com/tcgplayer/71/groups | Lorcana groups (4 KB) |
| last-updated.txt | https://tcgcsv.com/last-updated.txt | tcgcsv last-refresh timestamp |
| tcgcsv-robots.txt | https://tcgcsv.com/robots.txt | `Allow: /` |
| tcgcsv-home.html, faq.html | https://tcgcsv.com/ , https://tcgcsv.com/faq | site docs: daily ~20:00 UTC update, scrape etiquette, archive from 2024-02-08 |
| sor-products.json | https://tcgcsv.com/tcgplayer/79/23405/products | FULL Spark of Rebellion products (650 KB): 550 products, 544 cards, extendedData fields |
| sor-prices.json | https://tcgcsv.com/tcgplayer/79/23405/prices | FULL SOR prices (145 KB): 986 rows, Normal/Foil subTypeName |
| sor-products.csv | https://tcgcsv.com/tcgplayer/79/23405/ProductsAndPrices.csv | SOR combined CSV (386 KB) |
| hmw-products.json, hmw-prices.json | .../79/24812/products, /prices | newest set Homeworlds (1.16 MB / 133 KB) |
| tcgcsv-swu-counts.txt | derived by tools/dumpstar/count_tcgcsv_swu.py from all 34 groups' /products (~11 MB, not stored) | per-group product and card counts; total 9114 products / 8964 card products |
| off-cards-p2.json | https://admin.starwarsunlimited.com/api/cards?pagination[pageSize]=2 | official API default shape, 2 cards, all relations populated (182 KB) |
| off-en-meta.json, off-sor-meta.json, off-hmw-meta.json | same endpoint, pageSize=1 + filters | pagination.total: 10193 overall, 991 SOR, 954 HMW |
| off-single-uid.json | ...?locale=en&filters[cardUid][$eq]=6202608327 | single-card lookup by cardUid |
| off-cid.json | ...?locale=en&filters[cardId][$eq]=2579145458 | cardId groups printings of one card (3 rows) |
| off-lite-100.json | ...?locale=en&pagination[pageSize]=100&fields...&populate... | 100-card lite page, 5.6 MB uncompressed (~373 KB gzip) |
| off-sample-page40/80/102.json | same lite query, pagination[page]=40/80/102 | later pages: later sets, variant types |
| off-recent-updated.json | ...?sort[0]=updatedAt:desc&pageSize=3 | delta-sync handle |
| official-sor-pages/sor-page1..10.json | ...?filters[expansion][code][$eq]=SOR, 10 pages | complete official SOR (991 entries), used for the join test |
| off-expansions.json, off-variant-types.json, off-rarities.json, off-aspects.json | https://admin.starwarsunlimited.com/api/{expansions,variant-types,rarities,aspects} | all HTTP 404: no list endpoints for lookup tables |
| off-robots.txt | https://admin.starwarsunlimited.com/robots.txt | no restrictions (all commented out) |
| *.headers | - | response headers of the sibling file |
Not stored (size): full official catalog (10193 entries, about 570 MB uncompressed, about 38 MB gzip est.), all other tcgcsv groups' products/prices.

Added 2026-10-13 by sp-urhv3o (dumpstar 7), retrieved with User-Agent `dumpstar-sync/0.1 (+https://star.dumpster.cards)`:

| File | source_url | Shows |
|---|---|---|
| sor-wpp-products.json | https://tcgcsv.com/tcgplayer/79/23451/products | Spark of Rebellion: Weekly Play Promos, 20 products, Number `NN/20`; joins official serial `0160NNN` (Weekly Play) and `0135NNN` (Weekly Play Foil) |
| sor-wpp-prices.json | https://tcgcsv.com/tcgplayer/79/23451/prices | the same group's price rows |
| last-updated.txt | https://tcgcsv.com/last-updated.txt | `2026-10-12T20:05:44+0000`, no trailing newline |
| promo-23452/23453/23454/23455-products.json.gz, -prices.json.gz | https://tcgcsv.com/tcgplayer/79/{23452 PRE, 23453 JDG, 23454 EEP, 23455 OPP}/products and /prices | the promo groups the SOR event/prize printings are listed in: Prerelease Promo (PRE), Prerelease Judge and SS Judge (JDG), SS Participation/Top 8/Top 4/Finalist/Champion (OPP), and the Hyperspace Luke/Vader 1/2, 2/2 (EEP). Other sets' promos share these groups. |
