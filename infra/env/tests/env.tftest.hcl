# Plan-free assertions with a mocked provider: no credentials, no network.
mock_provider "aws" {
  mock_data "aws_caller_identity" {
    defaults = {
      account_id = "123456789012"
    }
  }
  mock_data "aws_subnets" {
    defaults = {
      ids = ["subnet-b", "subnet-a"]
    }
  }
}

variables {
  deploy_role_arn = "arn:aws:iam::123456789012:role/terraform-deploy"
  backup_bucket   = "dumpstar-backups-123456789012"
  replica_prefix  = "prod/tenants/db_concierge"
}

run "policy_names_only_slot_edge_secrets_and_the_replica_prefix" {
  command = plan

  assert {
    condition = alltrue([for s in jsondecode(aws_iam_role_policy.env.policy).Statement :
    contains(["SlotEdgeSecrets", "ReplicaRead", "ReplicaList"], s.Sid)]) && length(jsondecode(aws_iam_role_policy.env.policy).Statement) == 3
    error_message = "the inline policy must have exactly the three scoped statements"
  }

  assert {
    condition     = jsondecode(aws_iam_role_policy.env.policy).Statement[0].Resource == ["arn:aws:secretsmanager:us-west-2:123456789012:secret:dumpstar/env-*/edge-??????"] && jsondecode(aws_iam_role_policy.env.policy).Statement[0].Action == ["secretsmanager:GetSecretValue"]
    error_message = "secrets: GetSecretValue on dumpstar/env-*/edge only"
  }

  assert {
    condition     = jsondecode(aws_iam_role_policy.env.policy).Statement[1].Resource == ["arn:aws:s3:::dumpstar-backups-123456789012/prod/tenants/db_concierge/*"] && jsondecode(aws_iam_role_policy.env.policy).Statement[1].Action == ["s3:GetObject"]
    error_message = "replica read must be Get on the replica prefix only"
  }

  assert {
    condition     = jsondecode(aws_iam_role_policy.env.policy).Statement[2].Action == ["s3:ListBucket"] && jsondecode(aws_iam_role_policy.env.policy).Statement[2].Condition.StringLike["s3:prefix"] == ["prod/tenants/db_concierge", "prod/tenants/db_concierge/*"]
    error_message = "replica list must be conditioned on the replica prefix"
  }

  assert {
    condition     = aws_iam_role_policy_attachment.ssm.policy_arn == "arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore"
    error_message = "SSM managed instance core is the only managed policy"
  }
}

run "security_group_has_no_ingress" {
  command = plan

  assert {
    condition     = length(aws_security_group.env.ingress) == 0
    error_message = "the security group must carry no ingress rules"
  }

  assert {
    condition     = length(keys(resource.aws_vpc_security_group_egress_rule.https)) > 0
    error_message = "egress 443 must exist"
  }
}

run "outputs_cover_the_env_keys" {
  command = plan

  assert {
    condition     = output.instance_type == "t3.medium" && output.subnet == "subnet-a"
    error_message = "instance type defaults to t3.medium and the subnet is a default-VPC subnet"
  }
}

run "replica_prefix_is_never_all_of_prod" {
  command = plan
  variables {
    replica_prefix = "prod/tenants"
  }
  expect_failures = [var.replica_prefix]
}
