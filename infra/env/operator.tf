# The identity deploy/env.sh runs as (sp-avf7ko). spira-concierge assumes it; nothing else may.
# Every mutating action is bound to the dumpstar-env:slot tag, so the role cannot touch an
# instance or volume env.sh did not launch. Applied by the Concierge under an MFA session.
locals {
  region_arn      = "arn:aws:ec2:${var.aws_region}"
  env_subnet_arn  = "${local.region_arn}:${local.account_id}:subnet/${sort(data.aws_subnets.default.ids)[0]}"
  slot_tag_exists = { Null = { "aws:RequestTag/dumpstar-env:slot" = "false" } }
  slot_tagged     = { Null = { "ec2:ResourceTag/dumpstar-env:slot" = "false" } }
}

resource "aws_iam_role" "operator" {
  name                 = "dumpstar-env-operator"
  permissions_boundary = var.permissions_boundary_arn
  tags                 = { terraform-deploy-boundary = "true" }

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect    = "Allow"
      Action    = "sts:AssumeRole"
      Principal = { AWS = "arn:aws:iam::${local.account_id}:role/spira-concierge" }
    }]
  })
}

resource "aws_iam_role_policy" "operator" {
  name = "dumpstar-env-operator"
  role = aws_iam_role.operator.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Sid    = "LaunchTagged"
        Effect = "Allow"
        Action = ["ec2:RunInstances"]
        Resource = [
          "${local.region_arn}:${local.account_id}:instance/*",
          "${local.region_arn}:${local.account_id}:volume/*",
        ]
        Condition = local.slot_tag_exists
      },
      {
        # An ENI launched with an instance cannot be tagged, so it is bound to the env subnet.
        Sid       = "LaunchNetworkInterface"
        Effect    = "Allow"
        Action    = ["ec2:RunInstances"]
        Resource  = ["${local.region_arn}:${local.account_id}:network-interface/*"]
        Condition = { StringEquals = { "ec2:Subnet" = local.env_subnet_arn } }
      },
      {
        Sid      = "LaunchFromEnvImage"
        Effect   = "Allow"
        Action   = ["ec2:RunInstances"]
        Resource = ["${local.region_arn}::image/*"]
        Condition = { StringEquals = {
          "ec2:Owner"     = local.account_id
          "ec2:ImageType" = "machine"
        } }
      },
      {
        Sid      = "LaunchIntoEnvSubnet"
        Effect   = "Allow"
        Action   = ["ec2:RunInstances"]
        Resource = [local.env_subnet_arn]
      },
      {
        Sid      = "LaunchWithEnvSecurityGroup"
        Effect   = "Allow"
        Action   = ["ec2:RunInstances"]
        Resource = ["${local.region_arn}:${local.account_id}:security-group/${aws_security_group.env.id}"]
      },
      {
        Sid      = "TagOnCreate"
        Effect   = "Allow"
        Action   = ["ec2:CreateTags"]
        Resource = ["${local.region_arn}:${local.account_id}:*/*"]
        Condition = {
          StringEquals = { "ec2:CreateAction" = "RunInstances" }
        }
      },
      {
        Sid      = "RetagSlot"
        Effect   = "Allow"
        Action   = ["ec2:CreateTags"]
        Resource = ["${local.region_arn}:${local.account_id}:instance/*"]
        Condition = local.slot_tagged
      },
      {
        Sid       = "PassVmRole"
        Effect    = "Allow"
        Action    = ["iam:PassRole"]
        Resource  = [aws_iam_role.env.arn]
        Condition = { StringEquals = { "iam:PassedToService" = "ec2.amazonaws.com" } }
      },
      {
        Sid    = "ManageSlotInstances"
        Effect = "Allow"
        Action    = ["ec2:TerminateInstances", "ec2:StopInstances"]
        Resource  = ["${local.region_arn}:${local.account_id}:instance/*"]
        Condition = local.slot_tagged
      },
      {
        # SSM authorizes SendCommand against ssm:resourceTag/; ec2:ResourceTag/ is never set in
        # that request context, so the ec2 key denies every call (sp-s2zynj).
        Sid    = "SendCommandToSlotInstances"
        Effect = "Allow"
        Action = ["ssm:SendCommand"]
        Resource = [
          "${local.region_arn}:${local.account_id}:instance/*",
          "arn:aws:ssm:${var.aws_region}:${local.account_id}:managed-instance/*",
        ]
        Condition = { Null = { "ssm:resourceTag/dumpstar-env:slot" = "false" } }
      },
      {
        Sid       = "DeleteSlotVolumes"
        Effect    = "Allow"
        Action    = ["ec2:DeleteVolume"]
        Resource  = ["${local.region_arn}:${local.account_id}:volume/*"]
        Condition = local.slot_tagged
      },
      {
        Sid      = "RunShellScriptDocument"
        Effect   = "Allow"
        Action   = ["ssm:SendCommand"]
        Resource = ["arn:aws:ssm:${var.aws_region}::document/AWS-RunShellScript"]
      },
      {
        Sid      = "Observe"
        Effect   = "Allow"
        Action   = ["ec2:Describe*", "ssm:GetCommandInvocation", "ssm:ListCommandInvocations", "ssm:DescribeInstanceInformation"]
        Resource = ["*"]
      },
    ]
  })
}
