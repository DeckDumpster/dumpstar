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
  hostname              = "env-2.dumpster.cards"
  cloudflare_account_id = "acct"
  cloudflare_zone_id    = "zone"
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
      cloudflare_zero_trust_tunnel_cloudflared_config.this["env-2"].config.ingress[0].hostname == "env-2.dumpster.cards" &&
      cloudflare_zero_trust_tunnel_cloudflared_config.this["env-2"].config.ingress[1].service == "http_status:404"
    )
    error_message = "ingress must route the instance hostname and end in a catch-all 404"
  }
}

run "dns_is_a_proxied_cname_to_the_tunnel" {
  command = plan

  assert {
    condition     = cloudflare_dns_record.this["env-2"].type == "CNAME" && cloudflare_dns_record.this["env-2"].proxied && cloudflare_dns_record.this["env-2"].name == "env-2.dumpster.cards"
    error_message = "DNS must be a proxied CNAME on the instance hostname"
  }
}

run "nothing_sits_in_front_of_dumpstar_at_the_edge" {
  command = plan

  # Prod has no edge gate and dumpstar signs in itself, so the module may not declare an Access
  # application or any rule that answers /.well-known/*, /mcp, /api or /health before dumpstar.
  assert {
    condition = !anytrue([
      for f in fileset(path.module, "*.tf") :
      can(regex("(?m)^\\s*resource\\s+\"cloudflare_(zero_trust_access|ruleset|workers|page_rule|firewall|filter|waf)", file("${path.module}/${f}")))
    ])
    error_message = "infra/edge declares an edge-level gate (Access, ruleset, worker, page or firewall rule) that prod does not have"
  }
  assert {
    condition     = length(fileset(path.module, "*.tf")) > 0
    error_message = "the guard must see the module's files, or it proves nothing"
  }
}

run "pocketid_client_calls_back_to_the_hostname_and_is_group_limited" {
  command = plan

  assert {
    condition     = pocketid_client.this["env-2"].callback_urls == tolist(["https://env-2.dumpster.cards/auth/callback"])
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

run "a_two_label_slot_hostname_is_refused" {
  command = plan

  variables {
    hostname = "env-2.test.dumpster.cards"
  }

  expect_failures = [var.hostname]
}

run "unknown_hostname_is_refused" {
  command = plan

  variables {
    hostname = "env-9.dumpster.cards"
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

run "edge_secret_carries_the_generated_oidc_client_id" {
  command = apply

  assert {
    condition     = jsondecode(aws_secretsmanager_secret_version.edge["env-2"].secret_string).DUMPSTAR_OIDC_CLIENT_ID == pocketid_client.this["env-2"].id
    error_message = "DUMPSTAR_OIDC_CLIENT_ID must be the client's id: client_id is an unset input"
  }
  assert {
    condition     = output.pocketid_client_id == pocketid_client.this["env-2"].id
    error_message = "the pocketid_client_id output is the client's id"
  }
}

run "an_empty_secret_value_refuses_the_plan" {
  command = plan

  override_data {
    target = data.cloudflare_zero_trust_tunnel_cloudflared_token.this
    values = {
      token = ""
    }
  }
  override_resource {
    target = pocketid_client.this
    values = {
      id            = "client"
      client_secret = "secret"
    }
  }

  expect_failures = [aws_secretsmanager_secret_version.edge]
}

run "the_mcp_audience_is_registered_as_a_pocketid_api" {
  command = plan

  assert {
    condition     = terraform_data.mcp_api["env-2"].input.audience == "https://env-2.dumpster.cards/mcp"
    error_message = "the slot's MCP audience must be registered in Pocket ID as an API"
  }
  assert {
    condition     = !terraform_data.mcp_api["env-2"].input.keep
    error_message = "a slot's API entry is removed with its edge"
  }
  assert {
    condition     = strcontains(file("${path.module}/pocketid-api.sh"), "cimd-access") && strcontains(file("${path.module}/pocketid-api.sh"), "\"enabled\":true")
    error_message = "the registration must allow CIMD clients"
  }
}

run "prod_adopts_its_mcp_api_and_never_removes_it" {
  command = plan

  variables {
    instance = "prod"
    hostname = "star.dumpster.cards"
  }

  assert {
    condition     = terraform_data.mcp_api["prod"].input.audience == "https://star.dumpster.cards/mcp" && terraform_data.mcp_api["prod"].input.keep
    error_message = "prod's audience is the hand-made one and is kept on destroy"
  }
}
