# deploy/infra-config.sh reads these with `terraform output -json`.
output "region" {
  value = var.aws_region
}

output "lake_bucket" {
  value = aws_s3_bucket.lake.bucket
}

output "backup_bucket" {
  value = aws_s3_bucket.backups.bucket
}

output "lake_secret_name" {
  value = aws_secretsmanager_secret.lake.name
}

output "backup_secret_name" {
  value = aws_secretsmanager_secret.backup.name
}
