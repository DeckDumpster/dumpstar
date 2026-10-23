# The one Access group every instance's policy points at (infra/edge takes its id as
# access_group_id). It lives in a root of its own so applying or destroying an instance slot
# never touches it.
resource "cloudflare_zero_trust_access_group" "dumpstar_users" {
  account_id = var.cloudflare_account_id
  name       = "dumpstar-users"

  include = [{
    login_method = { id = var.access_idp_id }
  }]

  require = [{
    oidc = {
      identity_provider_id = var.access_idp_id
      claim_name           = "groups"
      claim_value          = var.pocketid_group_name
    }
  }]
}
