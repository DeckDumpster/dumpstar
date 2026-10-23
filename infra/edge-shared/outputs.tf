output "access_group_id" {
  description = "Value for access_group_id in infra/edge/common.tfvars."
  value       = cloudflare_zero_trust_access_group.dumpstar_users.id
}
