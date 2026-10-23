# One per DUMPSTAR_ENV_* key of deploy/env.sh; README.md maps them into env.conf.
output "instance_profile" {
  value = aws_iam_instance_profile.env.name
}

output "security_group" {
  value = aws_security_group.env.id
}

output "subnet" {
  value = sort(data.aws_subnets.default.ids)[0]
}

output "ami" {
  value = data.aws_ami.runner.id
}

output "instance_type" {
  value = var.instance_type
}

# DUMPSTAR_ENV_AWS_PROFILE names a profile with role_arn = this and source_profile = spira-concierge.
output "operator_role_arn" {
  value = aws_iam_role.operator.arn
}
