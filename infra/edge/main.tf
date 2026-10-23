data "aws_caller_identity" "current" {}

locals {
  lake_bucket = coalesce(var.lake_bucket, "dumpstar-lake-${data.aws_caller_identity.current.account_id}")
  # Every resource is keyed by the instance so an import block can address it.
  keys = toset([var.instance])
}

resource "terraform_data" "workspace_matches" {
  lifecycle {
    precondition {
      condition     = terraform.workspace == var.instance || terraform.workspace == "default"
      error_message = "instance ${var.instance} must be applied in its own workspace (terraform workspace select ${var.instance}); got ${terraform.workspace}"
    }
  }
}

# --- Cloudflare tunnel, ingress, DNS ---------------------------------------------------------

resource "random_id" "tunnel_secret" {
  for_each    = local.keys
  byte_length = 32
}

resource "cloudflare_zero_trust_tunnel_cloudflared" "this" {
  for_each   = local.keys
  account_id = var.cloudflare_account_id
  name       = "dumpstar-${var.instance}"
  config_src = "cloudflare"
  # An imported tunnel keeps the secret it was made with.
  tunnel_secret = random_id.tunnel_secret[each.key].b64_std

  lifecycle {
    ignore_changes = [tunnel_secret]
  }
}

resource "cloudflare_zero_trust_tunnel_cloudflared_config" "this" {
  for_each   = local.keys
  account_id = var.cloudflare_account_id
  tunnel_id  = cloudflare_zero_trust_tunnel_cloudflared.this[each.key].id

  config = {
    ingress = [
      {
        hostname = var.hostname
        service  = var.origin_service
      },
      { service = "http_status:404" },
    ]
  }
}

resource "cloudflare_dns_record" "this" {
  for_each = local.keys
  zone_id  = var.cloudflare_zone_id
  name     = var.hostname
  type     = "CNAME"
  content  = "${cloudflare_zero_trust_tunnel_cloudflared.this[each.key].id}.cfargotunnel.com"
  proxied  = true
  ttl      = 1
}

# --- Access ----------------------------------------------------------------------------------
# Deliberately none. Prod has no Cloudflare Access application: dumpstar does its own sign-in
# (Pocket ID OIDC for pages, MCP OAuth for /mcp, bearer for /api), and an edge login wall in
# front of a slot would turn /.well-known/*, /mcp, /api and /health into a 302 to a login page
# that MCP and API clients cannot pass. A slot is reachable exactly the way prod is.

# --- Pocket ID OIDC client -------------------------------------------------------------------

resource "pocketid_client" "this" {
  for_each             = local.keys
  name                 = "dumpstar-${var.instance}"
  callback_urls        = ["https://${var.hostname}/auth/callback"]
  logout_callback_urls = ["https://${var.hostname}/"]
  allowed_user_groups  = var.pocketid_group_ids
  pkce_enabled         = true
}

# --- S3 prefix per instance ------------------------------------------------------------------

resource "aws_s3_object" "prefix" {
  for_each = local.keys
  bucket   = local.lake_bucket
  key      = "instance/${var.instance}/"
  content  = ""
}

# --- Secrets ---------------------------------------------------------------------------------
# The tunnel token and OIDC client secret go to Secrets Manager, the mechanism the lake and
# backup credentials already use (deploy/infra-config.sh reads it into podman secrets). They
# are never an output, so no plan or apply log carries them.

data "cloudflare_zero_trust_tunnel_cloudflared_token" "this" {
  for_each   = local.keys
  account_id = var.cloudflare_account_id
  tunnel_id  = cloudflare_zero_trust_tunnel_cloudflared.this[each.key].id
}

resource "aws_secretsmanager_secret" "edge" {
  for_each = local.keys
  name     = "${var.secret_prefix}/${var.instance}/edge"
}

resource "aws_secretsmanager_secret_version" "edge" {
  for_each  = local.keys
  secret_id = aws_secretsmanager_secret.edge[each.key].id
  secret_string = jsonencode({
    CLOUDFLARED_TUNNEL_TOKEN    = data.cloudflare_zero_trust_tunnel_cloudflared_token.this[each.key].token
    DUMPSTAR_OIDC_CLIENT_ID     = pocketid_client.this[each.key].id
    DUMPSTAR_OIDC_CLIENT_SECRET = pocketid_client.this[each.key].client_secret
  })

  # client_id is an optional input of pocketid_client, null when unset: the generated
  # identifier is .id. An empty value must stop the apply here, not a launched VM.
  lifecycle {
    precondition {
      condition = alltrue([for v in [
        data.cloudflare_zero_trust_tunnel_cloudflared_token.this[each.key].token,
        pocketid_client.this[each.key].id,
        pocketid_client.this[each.key].client_secret,
      ] : v != null && v != ""])
      error_message = "the edge secret for ${var.instance} would carry an empty value (tunnel token, OIDC client id or OIDC client secret)"
    }
  }
}

# --- Pocket ID API for the MCP audience ------------------------------------------------------
# Pocket ID rejects an OAuth `resource` that is not a registered API ("The 'resource' or 'scope'
# parameter is invalid"), and Trozz/pocketid 2.4.2 has no resource type for one. The smallest
# mechanism that keeps it in Terraform is a terraform_data that runs pocketid-api.sh against
# Pocket ID's /api/apis. The script adopts an API already registered for the audience, so prod's
# hand-made entry (2026-10-20) is never duplicated; and prod's is never removed on destroy
# (keep = true), only a slot's. It reads POCKETID_API_TOKEN from the environment.

resource "terraform_data" "mcp_api" {
  for_each = local.keys
  input = {
    base_url = var.pocketid_url
    audience = "https://${var.hostname}/mcp"
    name     = "dumpstar MCP${each.key == "prod" ? "" : " ${each.key}"}"
    keep     = each.key == "prod"
  }

  provisioner "local-exec" {
    command = "bash '${path.module}/pocketid-api.sh' ensure '${self.input.base_url}' '${self.input.audience}' '${self.input.name}'"
  }

  provisioner "local-exec" {
    when    = destroy
    command = self.input.keep ? "echo 'pocketid-api.sh: keeping ${self.input.audience}'" : "bash '${path.module}/pocketid-api.sh' remove '${self.input.base_url}' '${self.input.audience}' '${self.input.name}'"
  }
}
