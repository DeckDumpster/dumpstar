# Plan-free assertions with a mocked provider: no credentials, no network.
mock_provider "aws" {
  mock_data "aws_iam_policy_document" {
    defaults = {
      json = "{\"Version\":\"2012-10-17\",\"Statement\":[]}"
    }
  }
  mock_data "aws_caller_identity" {
    defaults = {
      account_id = "123456789012"
    }
  }
}

variables {
  deploy_role_arn = "arn:aws:iam::123456789012:role/terraform-deploy"
}

override_resource {
  target          = aws_s3_bucket.lake
  override_during = plan
  values = {
    arn = "arn:aws:s3:::dumpstar-lake-123456789012"
  }
}

override_resource {
  target          = aws_s3_bucket.backups
  override_during = plan
  values = {
    arn = "arn:aws:s3:::dumpstar-backups-123456789012"
  }
}

run "buckets_are_named_distinct_private_and_versioned" {
  command = plan

  assert {
    condition     = startswith(aws_s3_bucket.lake.bucket, "dumpstar-lake-") && startswith(aws_s3_bucket.backups.bucket, "dumpstar-backups-")
    error_message = "buckets must carry the dumpstar-lake-/dumpstar-backups- prefixes"
  }
  assert {
    condition     = aws_s3_bucket.lake.bucket != aws_s3_bucket.backups.bucket
    error_message = "lake and backups must be different buckets"
  }
  assert {
    condition = alltrue([
      for p in [aws_s3_bucket_public_access_block.lake, aws_s3_bucket_public_access_block.backups] :
      alltrue([p.block_public_acls, p.block_public_policy, p.ignore_public_acls, p.restrict_public_buckets])
    ])
    error_message = "all public access must be blocked on both buckets"
  }
  assert {
    condition = alltrue([
      aws_s3_bucket_versioning.lake.versioning_configuration[0].status == "Enabled",
      aws_s3_bucket_versioning.backups.versioning_configuration[0].status == "Enabled",
    ])
    error_message = "both buckets must be versioned"
  }
}

run "lake_lifecycle_is_scoped_to_the_lake_bucket_and_tenant_zone" {
  command = plan

  assert {
    condition     = aws_s3_bucket_lifecycle_configuration.lake.bucket == aws_s3_bucket.lake.id
    error_message = "the lake lifecycle must attach to the lake bucket"
  }
  assert {
    condition     = aws_s3_bucket_lifecycle_configuration.backups.bucket == aws_s3_bucket.backups.id && aws_s3_bucket_lifecycle_configuration.backups.bucket != aws_s3_bucket.lake.id
    error_message = "the backups lifecycle must attach to the backups bucket only"
  }
  assert {
    condition = alltrue([
      for r in aws_s3_bucket_lifecycle_configuration.lake.rule :
      length(r.expiration) == 0 || (r.filter[0].prefix == "tenant/" && r.expiration[0].days == 90)
    ])
    error_message = "the only current-object expiry in the lake is tenant/ at 90 days"
  }
  assert {
    condition = alltrue([
      for r in aws_s3_bucket_lifecycle_configuration.backups.rule : length(r.expiration) == 0
    ])
    error_message = "no backup rule may expire current objects"
  }
}

run "credentials_are_scoped_to_their_own_bucket" {
  command = plan

  assert {
    condition = alltrue(flatten([
      for s in data.aws_iam_policy_document.lake.statement :
      [for r in s.resources : startswith(r, "arn:aws:s3:::dumpstar-lake-")]
    ]))
    error_message = "the lake policy must name the lake bucket only"
  }
  assert {
    condition = alltrue(flatten([
      for s in data.aws_iam_policy_document.backup.statement :
      [for r in s.resources : startswith(r, "arn:aws:s3:::dumpstar-backups-")]
    ]))
    error_message = "the backup policy must name the backups bucket only"
  }
  assert {
    condition = toset(flatten([for s in data.aws_iam_policy_document.backup.statement : s.actions])) == toset([
      "s3:ListBucket", "s3:GetObject", "s3:PutObject", "s3:DeleteObject",
    ])
    error_message = "the backup writer may not delete versions or touch bucket configuration"
  }
  assert {
    condition = alltrue([
      for u in [aws_iam_user.lake, aws_iam_user.backup] :
      u.tags["terraform-deploy-boundary"] == "true" && u.permissions_boundary == var.permissions_boundary_arn
    ])
    error_message = "both users must carry the deploy boundary and tag"
  }
}

run "tenant_user_reaches_only_tenant_and_lake_user_is_denied_it" {
  command = plan

  assert {
    condition = alltrue(flatten([
      for s in data.aws_iam_policy_document.tenant.statement :
      [for r in s.resources : startswith(r, "arn:aws:s3:::dumpstar-lake-")]
    ]))
    error_message = "the tenant policy must name the lake bucket only"
  }
  assert {
    condition = alltrue(flatten([
      for s in data.aws_iam_policy_document.tenant.statement :
      [for r in s.resources : endswith(r, "/tenant/*") || r == aws_s3_bucket.lake.arn]
      if s.effect != "Deny"
    ]))
    error_message = "every tenant allow is on tenant/ objects or the bucket listing (prefix-conditioned)"
  }
  assert {
    condition = alltrue([
      for s in data.aws_iam_policy_document.tenant.statement :
      length(s.condition) == 1 && anytrue([for c in s.condition : toset(c.values) == toset(["tenant/*"])])
      if s.effect != "Deny" && contains(s.actions, "s3:ListBucket")
    ])
    error_message = "the tenant listing must be conditioned on the tenant/ prefix"
  }
  assert {
    condition = toset(flatten([
      for s in data.aws_iam_policy_document.tenant.statement : s.resources if s.effect == "Deny" && contains(s.actions, "s3:*")
    ])) == toset(["arn:aws:s3:::dumpstar-lake-123456789012/raw/*", "arn:aws:s3:::dumpstar-lake-123456789012/lake/*"])
    error_message = "the tenant user must be denied raw/ and lake/"
  }
  assert {
    condition = toset(flatten([
      for s in data.aws_iam_policy_document.tenant.statement : s.actions if s.effect != "Deny"
    ])) == toset(["s3:ListBucket", "s3:GetObject", "s3:PutObject", "s3:DeleteObject"])
    error_message = "the tenant user may not touch bucket configuration or delete versions"
  }
  assert {
    condition = anytrue([
      for s in data.aws_iam_policy_document.lake.statement :
      s.effect == "Deny" && contains(s.actions, "s3:*") && contains(s.resources, "arn:aws:s3:::dumpstar-lake-123456789012/tenant/*")
    ])
    error_message = "the lake user must be denied tenant/ objects"
  }
  assert {
    condition = anytrue([
      for s in data.aws_iam_policy_document.lake.statement :
      s.effect == "Deny" && contains(s.actions, "s3:ListBucket") && anytrue([for c in s.condition : toset(c.values) == toset(["tenant/*"])])
    ])
    error_message = "the lake user must be denied listing tenant/"
  }
  assert {
    condition     = aws_iam_user.tenant.tags["terraform-deploy-boundary"] == "true" && aws_iam_user.tenant.permissions_boundary == var.permissions_boundary_arn
    error_message = "the tenant user must carry the deploy boundary and tag"
  }
}
