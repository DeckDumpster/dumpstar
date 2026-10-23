# Prod's tunnel, DNS and Pocket ID client were made by hand (sp-qmkwly). Importing them
# is what makes the plan on prod empty. Slots leave var.import_ids null and these never fire.
# Ids: terraform output / the dashboard; the Cloudflare ones are <scope id>/<resource id>.

import {
  for_each = var.import_ids == null ? {} : { (var.instance) = var.import_ids }
  to       = cloudflare_zero_trust_tunnel_cloudflared.this[each.key]
  id       = "${var.cloudflare_account_id}/${each.value.tunnel}"
}

import {
  for_each = var.import_ids == null ? {} : { (var.instance) = var.import_ids }
  to       = cloudflare_zero_trust_tunnel_cloudflared_config.this[each.key]
  id       = "${var.cloudflare_account_id}/${each.value.tunnel}"
}

import {
  for_each = var.import_ids == null ? {} : { (var.instance) = var.import_ids }
  to       = cloudflare_dns_record.this[each.key]
  id       = "${var.cloudflare_zone_id}/${each.value.dns_record}"
}

import {
  for_each = var.import_ids == null ? {} : { (var.instance) = var.import_ids }
  to       = pocketid_client.this[each.key]
  id       = each.value.pocketid_client
}
