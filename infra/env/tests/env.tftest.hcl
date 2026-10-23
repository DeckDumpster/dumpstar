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

run "operator_trust_names_only_spira_concierge" {
  command = plan

  assert {
    condition     = jsondecode(aws_iam_role.operator.assume_role_policy).Statement[0].Principal == { AWS = "arn:aws:iam::123456789012:role/spira-concierge" } && length(jsondecode(aws_iam_role.operator.assume_role_policy).Statement) == 1
    error_message = "only spira-concierge may assume dumpstar-env-operator"
  }
}

run "operator_passrole_is_limited_to_the_vm_role" {
  command = plan

  assert {
    condition = length([for s in jsondecode(aws_iam_role_policy.operator.policy).Statement : s if contains(s.Action, "iam:PassRole")]) == 1 && alltrue([for s in jsondecode(aws_iam_role_policy.operator.policy).Statement :
    s.Resource == [aws_iam_role.env.arn] && s.Condition.StringEquals["iam:PassedToService"] == "ec2.amazonaws.com" if contains(s.Action, "iam:PassRole")])
    error_message = "PassRole: dumpstar-env-vm only, to ec2.amazonaws.com"
  }
}

run "operator_mutations_carry_the_slot_tag_condition" {
  command = plan

  # Exempt: the ENI and image/subnet/SG launch resources (untaggable, bound by their own
  # conditions or ARNs), create-time tagging, PassRole, and the AWS-owned SSM document.
  assert {
    condition = alltrue([for s in jsondecode(aws_iam_role_policy.operator.policy).Statement :
      contains(keys(try(s.Condition, {})), "Null") || contains(["LaunchNetworkInterface", "LaunchFromEnvImage", "LaunchIntoEnvSubnet", "LaunchWithEnvSecurityGroup", "TagOnCreate", "PassVmRole", "RunShellScriptDocument", "Observe"], s.Sid)
    ])
    error_message = "every mutating statement must carry the dumpstar-env:slot Null condition"
  }

  assert {
    condition = alltrue([for s in jsondecode(aws_iam_role_policy.operator.policy).Statement :
      contains(["ec2:TerminateInstances", "ec2:StopInstances", "ec2:DeleteVolume"], s.Action[0]) ? contains(keys(s.Condition.Null), "ec2:ResourceTag/dumpstar-env:slot") : true
    ])
    error_message = "terminate, stop and delete-volume act only on slot-tagged resources"
  }

  # SSM authorizes SendCommand on an instance with ssm:resourceTag/, never ec2:ResourceTag/.
  assert {
    condition = length([for s in jsondecode(aws_iam_role_policy.operator.policy).Statement : s if contains(s.Action, "ssm:SendCommand") && s.Sid != "RunShellScriptDocument"]) == 1 && alltrue([for s in jsondecode(aws_iam_role_policy.operator.policy).Statement :
      s.Condition.Null == { "ssm:resourceTag/dumpstar-env:slot" = "false" } if contains(s.Action, "ssm:SendCommand") && s.Sid != "RunShellScriptDocument"
    ]) && alltrue([for s in jsondecode(aws_iam_role_policy.operator.policy).Statement : !contains(s.Action, "ssm:SendCommand") if length(try(s.Condition.Null, {})) > 0 && contains(keys(s.Condition.Null), "ec2:ResourceTag/dumpstar-env:slot")])
    error_message = "ssm:SendCommand on instances is conditioned on ssm:resourceTag/dumpstar-env:slot, in its own statement"
  }

  assert {
    condition = alltrue([for s in jsondecode(aws_iam_role_policy.operator.policy).Statement :
    s.Condition.StringEquals["ec2:CreateAction"] == "RunInstances" if s.Sid == "TagOnCreate"])
    error_message = "CreateTags on create is limited to RunInstances"
  }
}
