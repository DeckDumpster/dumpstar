# The VM every test-environment slot runs on (deploy/env.sh, sp-8i63uo). One role, one security
# group, shared by env-1..env-3; the VMs themselves are launched and terminated by env.sh.
data "aws_caller_identity" "current" {}

data "aws_vpc" "default" {
  default = true
}

data "aws_subnets" "default" {
  filter {
    name   = "vpc-id"
    values = [data.aws_vpc.default.id]
  }
}

data "aws_ami" "runner" {
  most_recent = true
  owners      = ["self"]
  name_regex  = "^ephemeral-ci-runner-"

  filter {
    name   = "name"
    values = [var.ami_name_pattern]
  }
}

locals {
  account_id = data.aws_caller_identity.current.account_id
  # Secrets Manager appends a random suffix to the name.
  slot_edge_secrets = "arn:aws:secretsmanager:${var.aws_region}:${local.account_id}:secret:dumpstar/env-*/edge-??????"
  bucket_arn        = "arn:aws:s3:::${var.backup_bucket}"
}

resource "aws_iam_role" "env" {
  name                 = "dumpstar-env-vm"
  permissions_boundary = var.permissions_boundary_arn
  tags                 = { terraform-deploy-boundary = "true" }

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect    = "Allow"
      Action    = "sts:AssumeRole"
      Principal = { Service = "ec2.amazonaws.com" }
    }]
  })
}

resource "aws_iam_role_policy_attachment" "ssm" {
  role       = aws_iam_role.env.name
  policy_arn = "arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore"
}

resource "aws_iam_role_policy" "env" {
  name = "dumpstar-env-vm"
  role = aws_iam_role.env.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Sid      = "SlotEdgeSecrets"
        Effect   = "Allow"
        Action   = ["secretsmanager:GetSecretValue"]
        Resource = [local.slot_edge_secrets]
      },
      {
        Sid      = "ReplicaRead"
        Effect   = "Allow"
        Action   = ["s3:GetObject"]
        Resource = ["${local.bucket_arn}/${var.replica_prefix}/*"]
      },
      {
        Sid       = "ReplicaList"
        Effect    = "Allow"
        Action    = ["s3:ListBucket"]
        Resource  = [local.bucket_arn]
        Condition = { StringLike = { "s3:prefix" = ["${var.replica_prefix}", "${var.replica_prefix}/*"] } }
      },
    ]
  })
}

resource "aws_iam_instance_profile" "env" {
  name = "dumpstar-env-vm"
  role = aws_iam_role.env.name
  tags = { terraform-deploy-boundary = "true" }
}

# No ingress rule exists: the tunnel and SSM both dial out.
resource "aws_security_group" "env" {
  name        = "dumpstar-env-vm"
  description = "dumpstar test-environment VM: no ingress, egress to the tunnel and AWS endpoints"
  vpc_id      = data.aws_vpc.default.id
  ingress     = [] # inline and empty: Terraform also removes a rule added by hand
}

resource "aws_vpc_security_group_egress_rule" "https" {
  security_group_id = aws_security_group.env.id
  description       = "HTTPS: SSM, Secrets Manager, S3, image pulls"
  ip_protocol       = "tcp"
  from_port         = 443
  to_port           = 443
  cidr_ipv4         = "0.0.0.0/0"
}

resource "aws_vpc_security_group_egress_rule" "tunnel_tcp" {
  security_group_id = aws_security_group.env.id
  description       = "cloudflared tunnel"
  ip_protocol       = "tcp"
  from_port         = 7844
  to_port           = 7844
  cidr_ipv4         = "0.0.0.0/0"
}

resource "aws_vpc_security_group_egress_rule" "tunnel_udp" {
  security_group_id = aws_security_group.env.id
  description       = "cloudflared tunnel (QUIC)"
  ip_protocol       = "udp"
  from_port         = 7844
  to_port           = 7844
  cidr_ipv4         = "0.0.0.0/0"
}
