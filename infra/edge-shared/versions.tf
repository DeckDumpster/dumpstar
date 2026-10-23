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
  }

  # One state for the one shared group (default workspace), apart from every instance's:
  # s3://<bucket>/dumpstar/edge-shared/terraform.tfstate.
  backend "s3" {
    key          = "dumpstar/edge-shared/terraform.tfstate"
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

# The token comes from the environment (CLOUDFLARE_API_TOKEN, Zero Trust edit), never a file.
provider "cloudflare" {}
