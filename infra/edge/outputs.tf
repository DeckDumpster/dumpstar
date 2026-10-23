# Names and ids only: nothing sensitive is an output.
output "hostname" {
  value = var.hostname
}

output "tunnel_id" {
  value = cloudflare_zero_trust_tunnel_cloudflared.this[var.instance].id
}

output "pocketid_client_id" {
  value = pocketid_client.this[var.instance].id
}

output "edge_secret_name" {
  value = aws_secretsmanager_secret.edge[var.instance].name
}

output "s3_prefix" {
  value = "s3://${local.lake_bucket}/${aws_s3_object.prefix[var.instance].key}"
}
