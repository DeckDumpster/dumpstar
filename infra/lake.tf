# The lake bucket (deploy/LAKE.md): raw/ and lake/ are the catalog zone, kept indefinitely;
# tenant/ is the tenant zone, 90 days (deploy/TENANT_ZONE.md). Everything here is
# reproducible from upstream, which is why it is a different bucket from the backups.
data "aws_caller_identity" "current" {}

locals {
  lake_bucket   = "dumpstar-lake-${data.aws_caller_identity.current.account_id}"
  backup_bucket = "dumpstar-backups-${data.aws_caller_identity.current.account_id}"
}

resource "aws_s3_bucket" "lake" {
  bucket = local.lake_bucket

  lifecycle {
    prevent_destroy = true
  }
}

resource "aws_s3_bucket_public_access_block" "lake" {
  bucket                  = aws_s3_bucket.lake.id
  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}

resource "aws_s3_bucket_server_side_encryption_configuration" "lake" {
  bucket = aws_s3_bucket.lake.id

  rule {
    apply_server_side_encryption_by_default {
      sse_algorithm = "AES256"
    }
  }
}

resource "aws_s3_bucket_versioning" "lake" {
  bucket = aws_s3_bucket.lake.id

  versioning_configuration {
    status = "Enabled"
  }
}

# There is deliberately no rule on raw/ or lake/. The only expiring prefix is tenant/.
resource "aws_s3_bucket_lifecycle_configuration" "lake" {
  bucket = aws_s3_bucket.lake.id

  rule {
    id     = "tenant-zone-retention"
    status = "Enabled"

    filter {
      prefix = "tenant/"
    }

    expiration {
      days = var.tenant_zone_days
    }

    noncurrent_version_expiration {
      noncurrent_days = 7
    }
  }

  rule {
    id     = "abort-incomplete-multipart"
    status = "Enabled"

    filter {}

    abort_incomplete_multipart_upload {
      days_after_initiation = 7
    }
  }

  depends_on = [aws_s3_bucket_versioning.lake]
}

data "aws_iam_policy_document" "lake_tls_only" {
  statement {
    sid       = "DenyInsecureTransport"
    effect    = "Deny"
    actions   = ["s3:*"]
    resources = [aws_s3_bucket.lake.arn, "${aws_s3_bucket.lake.arn}/*"]

    principals {
      type        = "*"
      identifiers = ["*"]
    }

    condition {
      test     = "Bool"
      variable = "aws:SecureTransport"
      values   = ["false"]
    }
  }
}

resource "aws_s3_bucket_policy" "lake" {
  bucket     = aws_s3_bucket.lake.id
  policy     = data.aws_iam_policy_document.lake_tls_only.json
  depends_on = [aws_s3_bucket_public_access_block.lake]
}

resource "aws_iam_user" "lake" {
  name                 = "dumpstar-lake"
  permissions_boundary = var.permissions_boundary_arn
  tags                 = { terraform-deploy-boundary = "true" }
}

# Landing, derive, ship and erase: list, read, write, and delete objects in this bucket.
# No bucket-level, lifecycle or version-delete action: retention is not this identity's to
# change (TENANT_ZONE.md §4a), and a delete leaves the version behind.
data "aws_iam_policy_document" "lake" {
  statement {
    actions   = ["s3:ListBucket"]
    resources = [aws_s3_bucket.lake.arn]
  }

  statement {
    actions   = ["s3:GetObject", "s3:PutObject", "s3:DeleteObject"]
    resources = ["${aws_s3_bucket.lake.arn}/*"]
  }

  # The tenant zone shares this bucket; only the tenant user reaches it (TENANT_ZONE.md).
  statement {
    sid       = "DenyTenantZoneObjects"
    effect    = "Deny"
    actions   = ["s3:*"]
    resources = ["${aws_s3_bucket.lake.arn}/tenant/*"]
  }

  statement {
    sid       = "DenyListingTenantZone"
    effect    = "Deny"
    actions   = ["s3:ListBucket"]
    resources = [aws_s3_bucket.lake.arn]

    condition {
      test     = "StringLike"
      variable = "s3:prefix"
      values   = ["tenant/*"]
    }
  }
}

resource "aws_iam_user_policy" "lake" {
  name   = "dumpstar-lake-rw"
  user   = aws_iam_user.lake.name
  policy = data.aws_iam_policy_document.lake.json
}

resource "aws_iam_access_key" "lake" {
  user = aws_iam_user.lake.name
}

resource "aws_secretsmanager_secret" "lake" {
  name        = "dumpstar/lake"
  description = "S3 credential for the dumpstar lake bucket (deploy/infra-config.sh reads it)"
}

resource "aws_secretsmanager_secret_version" "lake" {
  secret_id = aws_secretsmanager_secret.lake.id

  secret_string = jsonencode({
    AWS_ACCESS_KEY_ID     = aws_iam_access_key.lake.id
    AWS_SECRET_ACCESS_KEY = aws_iam_access_key.lake.secret
  })
}

resource "aws_iam_user" "tenant" {
  name                 = "dumpstar-tenant"
  permissions_boundary = var.permissions_boundary_arn
  tags                 = { terraform-deploy-boundary = "true" }
}

# The tenant zone (deploy/TENANT_ZONE.md): put/get/list/delete under tenant/ of the lake
# bucket and nothing else. raw/ and lake/ are denied outright, so no later allow can reach them.
data "aws_iam_policy_document" "tenant" {
  statement {
    sid       = "TenantZoneObjects"
    actions   = ["s3:GetObject", "s3:PutObject", "s3:DeleteObject"]
    resources = ["${aws_s3_bucket.lake.arn}/tenant/*"]
  }

  statement {
    sid       = "TenantZoneListingIsPrefixScoped"
    actions   = ["s3:ListBucket"]
    resources = [aws_s3_bucket.lake.arn]

    condition {
      test     = "StringLike"
      variable = "s3:prefix"
      values   = ["tenant/*"]
    }
  }

  statement {
    sid       = "DenyCatalogZoneObjects"
    effect    = "Deny"
    actions   = ["s3:*"]
    resources = ["${aws_s3_bucket.lake.arn}/raw/*", "${aws_s3_bucket.lake.arn}/lake/*"]
  }

  statement {
    sid       = "DenyListingCatalogZone"
    effect    = "Deny"
    actions   = ["s3:ListBucket"]
    resources = [aws_s3_bucket.lake.arn]

    condition {
      test     = "StringLike"
      variable = "s3:prefix"
      values   = ["raw/*", "lake/*"]
    }
  }
}

resource "aws_iam_user_policy" "tenant" {
  name   = "dumpstar-tenant-rw"
  user   = aws_iam_user.tenant.name
  policy = data.aws_iam_policy_document.tenant.json
}

resource "aws_iam_access_key" "tenant" {
  user = aws_iam_user.tenant.name
}

resource "aws_secretsmanager_secret" "tenant" {
  name        = "dumpstar/tenant"
  description = "S3 credential for the dumpstar tenant zone, tenant/ of the lake bucket (deploy/infra-config.sh reads it)"
}

resource "aws_secretsmanager_secret_version" "tenant" {
  secret_id = aws_secretsmanager_secret.tenant.id

  secret_string = jsonencode({
    AWS_ACCESS_KEY_ID     = aws_iam_access_key.tenant.id
    AWS_SECRET_ACCESS_KEY = aws_iam_access_key.tenant.secret
  })
}
