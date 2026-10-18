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
  description = "Boundary the terraform-deploy role requires on every IAM user or role it creates."
  default     = "arn:aws:iam::189923011121:policy/terraform-deploy-boundary"
}

variable "tenant_zone_days" {
  type        = number
  description = "Days an object under tenant/ is kept (deploy/TENANT_ZONE.md §2: 90)."
  default     = 90
}

variable "backup_noncurrent_days" {
  type        = number
  description = "Days a superseded or deleted backup version is kept before it expires."
  default     = 400
}
