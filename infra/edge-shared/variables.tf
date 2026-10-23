variable "aws_region" {
  type    = string
  default = "us-west-2"
}

variable "deploy_role_arn" {
  type        = string
  description = "DeployRoleArn output of TerraformBootstrap (role terraform-deploy)."
}

variable "cloudflare_account_id" {
  type = string
}

variable "access_idp_id" {
  type        = string
  description = "Id of the Pocket ID identity provider configured in Zero Trust."
}

variable "pocketid_group_name" {
  type        = string
  description = "Name of the Pocket ID group whose members may reach dumpstar; the IdP sends it in the groups claim."
  default     = "dumpstar-users"
}
