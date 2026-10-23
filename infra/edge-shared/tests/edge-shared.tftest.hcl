# Plan-free assertions with mocked providers: no credentials, no network.
mock_provider "aws" {}
mock_provider "cloudflare" {}

variables {
  deploy_role_arn       = "arn:aws:iam::123456789012:role/terraform-deploy"
  cloudflare_account_id = "acct"
  access_idp_id         = "idp"
}

run "group_includes_the_idp_and_requires_the_pocket_id_group_claim" {
  command = plan

  assert {
    condition     = cloudflare_zero_trust_access_group.dumpstar_users.name == "dumpstar-users"
    error_message = "the group is named dumpstar-users"
  }
  assert {
    condition = (
      length(cloudflare_zero_trust_access_group.dumpstar_users.include) == 1 &&
      cloudflare_zero_trust_access_group.dumpstar_users.include[0].login_method.id == "idp"
    )
    error_message = "include must be the login method of the Pocket ID IdP"
  }
  assert {
    condition = (
      length(cloudflare_zero_trust_access_group.dumpstar_users.require) == 1 &&
      cloudflare_zero_trust_access_group.dumpstar_users.require[0].oidc.identity_provider_id == "idp" &&
      cloudflare_zero_trust_access_group.dumpstar_users.require[0].oidc.claim_name == "groups" &&
      cloudflare_zero_trust_access_group.dumpstar_users.require[0].oidc.claim_value == "dumpstar-users"
    )
    error_message = "require must be the OIDC claim groups = dumpstar-users from that IdP"
  }
}

run "the_group_id_is_an_output" {
  command = apply

  assert {
    condition     = output.access_group_id == cloudflare_zero_trust_access_group.dumpstar_users.id
    error_message = "the root outputs the group id for infra/edge"
  }
}
