# infra/edge — one instance's edge and identity (sp-1vuuva)

Terraform for what stands between a browser and a dumpstar origin, per instance, from the design
"Disposable test environments from any build": prod (`star.dumpster.cards`) and the
pre-provisioned test slots `env-1..env-3` (`env-N.dumpster.cards`). Each instance is its own
**workspace** with its own state (`env:/<instance>/dumpstar/edge/terraform.tfstate`), so a slot
applies and destroys alone.

**Slot hostnames are one label under the zone.** The zone's free Universal SSL certificate covers
`dumpster.cards` and `*.dumpster.cards`, and a wildcard covers exactly one label, so
`env-N.test.dumpster.cards` gets no edge certificate and never answers. The `hostname` validation
refuses any slot name with more than one label below the zone. `env-N.test.dumpster.cards` remains
the preferred scheme (operator verdict, sp-z14vrm) once the zone has a certificate covering
`*.test.dumpster.cards`; the single-label validation is what has to change at that point.

Per instance:

- Cloudflare tunnel `dumpstar-<instance>`, its ingress (hostname → `origin_service`, else 404)
  and a proxied CNAME to `<tunnel>.cfargotunnel.com`.
- No Cloudflare Access application or policy, on a slot or on prod: prod has none, and an edge
  login wall would answer `/.well-known/*`, `/mcp`, `/api` and `/health` with a 302 before
  dumpstar does, which MCP and API clients cannot pass. dumpstar signs in itself (Pocket ID OIDC).
- Pocket ID OIDC client `dumpstar-<instance>` (callback `https://<host>/auth/callback`, PKCE,
  limited to the required Pocket ID group), through the registry's maintained community provider
  `Trozz/pocketid` — no bespoke tool.
- S3 prefix `instance/<instance>/` in the lake bucket.
- Secrets Manager `dumpstar/<instance>/edge`: `CLOUDFLARED_TUNNEL_TOKEN`, `DUMPSTAR_OIDC_CLIENT_ID`,
  `DUMPSTAR_OIDC_CLIENT_SECRET`. The same mechanism as `dumpstar/lake`;
  there are no secret outputs, so no log carries one.

## Apply order

The Access group every instance's policy points at is not made here: it is the one shared group
declared in `../edge-shared` (group `dumpstar-users`: include the Pocket ID IdP's login method,
require the OIDC claim `groups = dumpstar-users` from it). So:

1. `cd infra/edge-shared && bash run.sh apply` — once, in its own state, never touched by an
   instance's apply or destroy.
2. Put its output (`bash run.sh output access_group_id`) in `common.tfvars` as `access_group_id`.
3. Then each instance: `bash run.sh <instance> apply` here.

```bash
cd infra/edge
terraform test                           # hermetic, mocked providers
bash run.sh env-1 plan
bash run.sh env-1 apply
```

## Importing prod (once)

Prod's tunnel, DNS record and Pocket ID client were made by hand
(`deploy/README.md`, sp-qmkwly). Copy `prod.import.tfvars.example` to `prod.import.tfvars`, fill
the three ids, then `bash run.sh prod import-plan`; the plan must show only the imports and no
change. Apply, then delete the file: the `import` blocks only fire while `import_ids` is set.
Two things to read in that first plan: `tunnel_secret` is ignored (an imported tunnel keeps its
own), and anything the dashboard set that the module does not shows as
a diff to settle in `variables.tf`, not to apply over.

Providers read `CLOUDFLARE_API_TOKEN` and `POCKETID_API_TOKEN` from the environment.

## Pocket ID API for the MCP audience

Pocket ID accepts an OAuth `resource` only if it is a registered API, and `Trozz/pocketid` has no
resource type for one. `terraform_data.mcp_api` therefore runs `pocketid-api.sh` (Pocket ID's
`/api/apis`, with `POCKETID_API_TOKEN`) to register `https://<hostname>/mcp` with CIMD clients
allowed, which is what lets Claude Code sign in. An API already registered for the audience is
adopted, not duplicated (prod's hand-made `dumpstar MCP`); a slot's is deleted on destroy, prod's
never is. `tests/ci/pocketid_api_test.sh` exercises the script against a stub.
