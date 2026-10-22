# infra/ — the lake and backup buckets (sp-zcne5d)

Terraform for dumpstar's two S3 buckets, in the same account (backups move to the CI account
when Ryan settles the ask filed with sp-zcne5d; only `backups.tf` changes):

- `dumpstar-lake-<account>`: private, SSE-S3, versioned, TLS-only. The one expiring prefix is
  `tenant/` at 90 days; `raw/` and `lake/` have no rule. User `dumpstar-lake` (denied `tenant/`),
  secret `dumpstar/lake`. User `dumpstar-tenant` (put/get/list/delete under `tenant/` only; denied
  `raw/` and `lake/`), secret `dumpstar/tenant`: the tenant-zone credential ship and erase use.
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

## infra/env — the test-environment VM (sp-q8iz7i)

`infra/env` is its own root (own state key, `dumpstar/infra/env/terraform.tfstate`) for what
`deploy/env.sh up` launches, shared by env-1..env-3: role and instance profile
`dumpstar-env-vm` (`secretsmanager:GetSecretValue` on `dumpstar/env-*/edge` only; `s3:GetObject`
and a prefix-conditioned `s3:ListBucket` on the concierge replica prefix only;
`AmazonSSMManagedInstanceCore`), a security group with no ingress (egress 443 and the tunnel's
7844), a default-VPC subnet, and the newest `ephemeral-ci-runner-*` AMI. The AMI must carry the
aws CLI and the SSM agent; if it lacks either, add them to ephemeral-ci's substrate.

```bash
cd infra/env
terraform init -backend-config=backend.tfvars        # backend.tfvars: bucket, role_arn (gitignored)
terraform test                                       # hermetic, mocked provider
terraform apply -var deploy_role_arn=<DeployRoleArn> \
  -var replica_prefix=prod/tenants/<concierge database id>   # MFA session, by the Concierge
```

`replica_prefix` has no default and is refused if it is `prod/tenants` itself. Fill
`~/.config/dumpstar/env.conf` from the outputs:

| env.conf key | `terraform output -raw` |
| --- | --- |
| `DUMPSTAR_ENV_INSTANCE_PROFILE` | `instance_profile` |
| `DUMPSTAR_ENV_SECURITY_GROUP` | `security_group` |
| `DUMPSTAR_ENV_SUBNET` | `subnet` |
| `DUMPSTAR_ENV_AMI` | `ami` |
| `DUMPSTAR_ENV_INSTANCE_TYPE` | `instance_type` (variable, default `t3.medium`) |
