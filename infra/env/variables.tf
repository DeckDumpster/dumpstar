variable "aws_region" {
  type    = string
  default = "us-west-2"
}

variable "deploy_role_arn" {
  type        = string
  description = "DeployRoleArn output of TerraformBootstrap (role terraform-deploy)."
}

variable "permissions_boundary_arn" {
  type        = string
  description = "Boundary the terraform-deploy role requires on every IAM role it creates."
  default     = "arn:aws:iam::189923011121:policy/terraform-deploy-boundary"
}

variable "instance_type" {
  type        = string
  description = "EC2 instance type of an environment VM."
  default     = "t3.medium"
}

variable "backup_bucket" {
  type        = string
  description = "The Litestream bucket the VM reads the concierge replica from."
  default     = "dumpstar-backups-189923011121"
}

variable "replica_prefix" {
  type        = string
  description = "Key prefix of the concierge tenant's replica in the backup bucket, no leading or trailing slash (prod/tenants/<concierge database id>). Never the whole prod/tenants."
  validation {
    condition     = can(regex("^[A-Za-z0-9._-]+(/[A-Za-z0-9._-]+)+$", var.replica_prefix)) && var.replica_prefix != "prod/tenants"
    error_message = "replica_prefix must be a path of at least two segments naming one tenant, never prod/tenants itself."
  }
}

variable "ami_name_pattern" {
  type        = string
  description = "Name of the ephemeral-ci runner image, latest wins."
  default     = "ephemeral-ci-runner-*"
}
