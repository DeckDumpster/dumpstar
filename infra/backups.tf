# The Litestream bucket: tenant databases and the user registry, the only irreplaceable data
# dumpstar has (deploy/litestream.yml). It shares nothing with the lake, so no lake rule can
# reach it. Same account as the lake bucket for now (the CI-account split is pending Ryan's
# answer, sp-zcne5d); the rest of this file does not change when it moves.
resource "aws_s3_bucket" "backups" {
  bucket = local.backup_bucket

  lifecycle {
    prevent_destroy = true
  }
}

resource "aws_s3_bucket_public_access_block" "backups" {
  bucket                  = aws_s3_bucket.backups.id
  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}

resource "aws_s3_bucket_server_side_encryption_configuration" "backups" {
  bucket = aws_s3_bucket.backups.id

  rule {
    apply_server_side_encryption_by_default {
      sse_algorithm = "AES256"
    }
  }
}

resource "aws_s3_bucket_versioning" "backups" {
  bucket = aws_s3_bucket.backups.id

  versioning_configuration {
    status = "Enabled"
  }
}

# Only superseded versions expire, never current objects: Litestream prunes its own retention
# (deploy/litestream.yml) and a delete it issues leaves a version for this window.
resource "aws_s3_bucket_lifecycle_configuration" "backups" {
  bucket = aws_s3_bucket.backups.id

  rule {
    id     = "expire-noncurrent"
    status = "Enabled"

    filter {}

    noncurrent_version_expiration {
      noncurrent_days = var.backup_noncurrent_days
    }

    abort_incomplete_multipart_upload {
      days_after_initiation = 7
    }
  }

  depends_on = [aws_s3_bucket_versioning.backups]
}

data "aws_iam_policy_document" "backups_tls_only" {
  statement {
    sid       = "DenyInsecureTransport"
    effect    = "Deny"
    actions   = ["s3:*"]
    resources = [aws_s3_bucket.backups.arn, "${aws_s3_bucket.backups.arn}/*"]

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

resource "aws_s3_bucket_policy" "backups" {
  bucket     = aws_s3_bucket.backups.id
  policy     = data.aws_iam_policy_document.backups_tls_only.json
  depends_on = [aws_s3_bucket_public_access_block.backups]
}

resource "aws_iam_user" "backup" {
  name                 = "dumpstar-backup"
  permissions_boundary = var.permissions_boundary_arn
  tags                 = { terraform-deploy-boundary = "true" }
}

# Litestream cannot be strictly write-only: replicating reads its own LTX files back to
# compact and lists the prefix to resume. The writer therefore gets put/get/list/delete on
# objects, and nothing that can destroy history: no DeleteObjectVersion, so every overwrite
# or delete leaves a version; no bucket, versioning or lifecycle action. Restore uses the
# operator's own identity (law-off-box-backups-are-private-and-write-only).
data "aws_iam_policy_document" "backup" {
  statement {
    actions   = ["s3:ListBucket"]
    resources = [aws_s3_bucket.backups.arn]
  }

  statement {
    actions   = ["s3:GetObject", "s3:PutObject", "s3:DeleteObject"]
    resources = ["${aws_s3_bucket.backups.arn}/*"]
  }
}

resource "aws_iam_user_policy" "backup" {
  name   = "dumpstar-backups-rw"
  user   = aws_iam_user.backup.name
  policy = data.aws_iam_policy_document.backup.json
}

resource "aws_iam_access_key" "backup" {
  user = aws_iam_user.backup.name
}

resource "aws_secretsmanager_secret" "backup" {
  name        = "dumpstar/backup"
  description = "S3 credential for the dumpstar backups bucket (deploy/infra-config.sh reads it)"
}

resource "aws_secretsmanager_secret_version" "backup" {
  secret_id = aws_secretsmanager_secret.backup.id

  secret_string = jsonencode({
    AWS_ACCESS_KEY_ID     = aws_iam_access_key.backup.id
    AWS_SECRET_ACCESS_KEY = aws_iam_access_key.backup.secret
  })
}
