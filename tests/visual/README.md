# Browser specs

Playwright specs that drive the app served by a throwaway `--test` container
instance seeded from the committed fixture. They assert **properties** — a
route renders, holds still once settled, a grid wraps to what fits, the
collection page renders a window rather than every row. There are no pixel
baselines and no approval step (sp-vcwvbq): a full-page screenshot diff caught
no real defect and forced every UI change into a re-record loop. Before/after
images belong on the pull request as review evidence, not in a gate.

## Running it

```bash
bash tests/visual/run.sh              # stand up an instance, run, tear down
bash tests/visual/run.sh --keep       # ...and leave the instance up
DUMPSTAR_BASE_URL=http://localhost:8099 bash tests/visual/playwright.sh
```

`run.sh` refuses to run against `prod`. `deploy/ci.sh` runs the specs against
the container it already started for the smoke test. After a failure,
`(cd tests/visual && npm run report)` opens the HTML report.

## What is covered

- `routes.spec.ts`: every entry in `routes.json` loads at both viewports and
  its capture size is stable after `settle()`. Adding a `+page.svelte` without
  adding it to `routes.json` fails `every route in the app is covered`; a page
  that needs no check goes in `unrepresented` with a reason. A route that
  settles late needs a `waitFor` in the manifest.
- `browse-grid.spec.ts`: the `/browse` set grid wraps to what fits.
- `collection-window.spec.ts`: `/collection` renders a viewport-sized window of
  a large result, with no pager.

Viewports live in `playwright.config.ts` (`desktop-1440`, `mobile-768`).

## Not the intents harness

`tests/ui` needs an `ANTHROPIC_API_KEY` and is non-deterministic by design;
this directory is offline Playwright and belongs in CI.
