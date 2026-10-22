# Plan-free assertions with mocked providers: no credentials, no network.
mock_provider "aws" {
  mock_data "aws_caller_identity" {
    defaults = {
      account_id = "123456789012"
    }
  }
}
mock_provider "cloudflare" {
  mock_data "cloudflare_zero_trust_tunnel_cloudflared_token" {
    defaults = {
      token = "mock-token"
    }
  }
}
mock_provider "pocketid" {}
mock_provider "random" {}

variables {
  deploy_role_arn       = "arn:aws:iam::123456789012:role/terraform-deploy"
  instance              = "env-2"
  hostname              = "env-2.test.dumpster.cards"
  cloudflare_account_id = "acct"
  cloudflare_zone_id    = "zone"
  access_team_name      = "team"
  access_idp_id         = "idp"
  access_group_id       = "grp"
  pocketid_group_ids    = ["g1"]
}

run "tunnel_ingress_routes_the_hostname_then_404s" {
  command = plan

  assert {
    condition     = cloudflare_zero_trust_tunnel_cloudflared.this["env-2"].name == "dumpstar-env-2"
    error_message = "the tunnel is named for its instance"
  }
  assert {
    condition = (
      cloudflare_zero_trust_tunnel_cloudflared_config.this["env-2"].config.ingress[0].hostname == "env-2.test.dumpster.cards" &&
      cloudflare_zero_trust_tunnel_cloudflared_config.this["env-2"].config.ingress[1].service == "http_status:404"
    )
    error_message = "ingress must route the instance hostname and end in a catch-all 404"
  }
}

run "dns_is_a_proxied_cname_to_the_tunnel" {
  command = plan

  assert {
    condition     = cloudflare_dns_record.this["env-2"].type == "CNAME" && cloudflare_dns_record.this["env-2"].proxied && cloudflare_dns_record.this["env-2"].name == "env-2.test.dumpster.cards"
    error_message = "DNS must be a proxied CNAME on the instance hostname"
  }
}

run "access_app_requires_the_group_on_the_hostname" {
  command = plan

  assert {
    condition     = cloudflare_zero_trust_access_application.this["env-2"].domain == "env-2.test.dumpster.cards" && cloudflare_zero_trust_access_application.this["env-2"].type == "self_hosted"
    error_message = "the Access application covers exactly the instance hostname"
  }
  assert {
    condition     = cloudflare_zero_trust_access_policy.this["env-2"].decision == "allow" && tolist(cloudflare_zero_trust_access_policy.this["env-2"].include)[0].group.id == "grp"
    error_message = "the policy allows only the required group"
  }
  assert {
    condition     = toset(cloudflare_zero_trust_access_application.this["env-2"].allowed_idps) == toset(["idp"])
    error_message = "only the Pocket ID identity provider may sign in"
  }
}

run "pocketid_client_calls_back_to_the_hostname_and_is_group_limited" {
  command = plan

  assert {
    condition     = pocketid_client.this["env-2"].callback_urls == tolist(["https://env-2.test.dumpster.cards/auth/callback"])
    error_message = "the callback is the instance's own"
  }
  assert {
    condition     = pocketid_client.this["env-2"].pkce_enabled && toset(pocketid_client.this["env-2"].allowed_user_groups) == toset(["g1"])
    error_message = "PKCE on and limited to the required group"
  }
}

run "s3_prefix_and_secret_are_per_instance" {
  command = plan

  assert {
    condition     = aws_s3_object.prefix["env-2"].key == "instance/env-2/" && aws_s3_object.prefix["env-2"].bucket == "dumpstar-lake-123456789012"
    error_message = "one S3 prefix per instance in the lake bucket"
  }
  assert {
    condition     = aws_secretsmanager_secret.edge["env-2"].name == "dumpstar/env-2/edge"
    error_message = "secrets live under dumpstar/<instance>/edge"
  }
}

run "prod_is_its_own_instance" {
  command = plan

  variables {
    instance = "prod"
    hostname = "star.dumpster.cards"
  }

  assert {
    condition     = cloudflare_zero_trust_tunnel_cloudflared.this["prod"].name == "dumpstar-prod" && cloudflare_dns_record.this["prod"].name == "star.dumpster.cards"
    error_message = "prod is dumpstar-prod on star.dumpster.cards"
  }
}

run "a_slot_cannot_take_another_instances_hostname" {
  command = plan

  variables {
    instance = "env-1"
  }

  expect_failures = [var.hostname]
}

run "unknown_hostname_is_refused" {
  command = plan

  variables {
    hostname = "env-9.test.dumpster.cards"
  }

  expect_failures = [var.hostname]
}

run "unknown_instance_is_refused" {
  command = plan

  variables {
    instance = "env-4"
  }

  expect_failures = [var.instance]
}
