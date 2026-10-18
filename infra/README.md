# infra/ — the lake and backup buckets (sp-zcne5d)

Terraform for dumpstar's two S3 buckets, in the same account (backups move to the CI account
when Ryan settles the ask filed with sp-zcne5d; only `backups.tf` changes):

- `dumpstar-lake-<account>`: private, SSE-S3, versioned, TLS-only. The one expiring prefix is
  `tenant/` at 90 days; `raw/` and `lake/` have no rule. User `dumpstar-lake`, secret
  `dumpstar/lake`.
- `dumpstar-backups-<account>`: the Litestream target. Private, SSE-S3, versioned, TLS-only,
  only noncurrent versions expire (400 days). User `dumpstar-backup` (put/get/list/delete on
  objects; no version delete, no bucket, versioning or lifecycle action), secret
  `dumpstar/backup`. Litestream reads its remote back, so the key cannot be write-only.

```bash
cd infra
terraform init -backend-config="bucket=<state-bucket>" -backend-config="role_arn=<deploy-role>"
terraform test                                          # hermetic, mocked provider
terraform apply -var deploy_role_arn=<DeployRoleArn>    # MFA session
bash ../deploy/infra-config.sh prod --secrets           # lake.env, litestream.env, podman secrets
```

The `dumpstar` prefix must be in the terraform-deploy role's `TERRAFORM_NAME_PREFIXES`
(ephemeral-ci `TerraformBootstrap`) before the apply can create these names.
