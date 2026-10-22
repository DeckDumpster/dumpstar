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

resource "cloudflare_zero_trust_access_policy" "this" {
  for_each   = local.keys
  account_id = var.cloudflare_account_id
  name       = "dumpstar-${var.instance} required group"
  decision   = "allow"

  include = [{
    group = { id = var.access_group_id }
  }]
}

resource "cloudflare_zero_trust_access_application" "this" {
  for_each                  = local.keys
  account_id                = var.cloudflare_account_id
  name                      = "dumpstar-${var.instance}"
  type                      = "self_hosted"
  domain                    = var.hostname
  session_duration          = var.access_session_duration
  allowed_idps              = [var.access_idp_id]
  auto_redirect_to_identity = true

  policies = [{
    id         = cloudflare_zero_trust_access_policy.this[each.key].id
    precedence = 1
  }]
}

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
    DUMPSTAR_OIDC_CLIENT_ID     = pocketid_client.this[each.key].client_id
    DUMPSTAR_OIDC_CLIENT_SECRET = pocketid_client.this[each.key].client_secret
    DUMPSTAR_ACCESS_AUD         = cloudflare_zero_trust_access_application.this[each.key].aud
  })
}
