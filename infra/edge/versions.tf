terraform {
  required_version = ">= 1.10"

  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "6.69.1"
    }
    cloudflare = {
      source  = "cloudflare/cloudflare"
      version = "5.28.1"
    }
    # Pocket ID has no vendor provider; Trozz/pocketid is the registry's maintained one
    # (tagged releases, v2.x, manages OIDC clients and groups). No bespoke tool is needed.
    pocketid = {
      source  = "Trozz/pocketid"
      version = "2.4.2"
    }
    random = {
      source  = "hashicorp/random"
      version = "3.8.1"
    }
  }

  # One state per instance: the S3 backend keys each workspace at
  # env:/<workspace>/dumpstar/edge/terraform.tfstate. Workspaces: prod, env-1, env-2, env-3.
  backend "s3" {
    key          = "dumpstar/edge/terraform.tfstate"
    region       = "us-west-2"
    encrypt      = true
    use_lockfile = true
  }
}

provider "aws" {
  region = var.aws_region

  assume_role {
    role_arn = var.deploy_role_arn
  }
}

# Tokens come from the environment, never from a variable or a file in git:
#   CLOUDFLARE_API_TOKEN   Zero Trust edit + DNS edit + Tunnel edit
#   POCKETID_API_TOKEN     a Pocket ID API key (admin)
provider "cloudflare" {}

provider "pocketid" {
  base_url = var.pocketid_url
}
