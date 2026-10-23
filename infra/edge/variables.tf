variable "aws_region" {
  type    = string
  default = "us-west-2"
}

variable "deploy_role_arn" {
  type        = string
  description = "DeployRoleArn output of TerraformBootstrap (role terraform-deploy)."
}

variable "instance" {
  type        = string
  description = "prod, or a pre-provisioned test slot env-1..env-3. Must equal the Terraform workspace."

  validation {
    condition     = contains(["prod", "env-1", "env-2", "env-3"], var.instance)
    error_message = "instance must be prod, env-1, env-2 or env-3"
  }
}

variable "hostname" {
  type        = string
  description = "Public hostname: star.dumpster.cards for prod, env-N.dumpster.cards for a slot."

  validation {
    condition     = var.hostname == (var.instance == "prod" ? "star.dumpster.cards" : "${var.instance}.dumpster.cards")
    error_message = "hostname must be star.dumpster.cards for prod, or <instance>.dumpster.cards for a slot: exactly one label below the zone, because Cloudflare Universal SSL covers only *.dumpster.cards and issues no certificate for a two-label name such as env-N.test.dumpster.cards"
  }
}

variable "origin_service" {
  type        = string
  description = "What the tunnel forwards to on the host that runs cloudflared."
  default     = "http://127.0.0.1:8091"
}

variable "cloudflare_account_id" {
  type = string
}

variable "cloudflare_zone_id" {
  type        = string
  description = "Zone of dumpster.cards."
}

variable "pocketid_url" {
  type    = string
  default = "https://id.gantt.pub"
}

variable "pocketid_group_ids" {
  type        = list(string)
  description = "Pocket ID group ids allowed on the client (the required group, dumpstar-users)."
}

variable "secret_prefix" {
  type    = string
  default = "dumpstar"
}

variable "lake_bucket" {
  type        = string
  description = "Lake bucket name (output lake_bucket of ../). Defaults to the account's dumpstar-lake-<account>."
  default     = null
}

# Prod's resources exist already; set this once to import them (prod.import.tfvars). Leave null
# for a slot. The ids are the tunnel id, the DNS record id and the Pocket ID client id.
variable "import_ids" {
  type = object({
    tunnel          = string
    dns_record      = string
    pocketid_client = string
  })
  default = null
}
