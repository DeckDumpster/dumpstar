<script lang="ts">
	import type { OtherPrinting } from '$lib/printings';

	// Navigation only: where else this card is printed. Each entry opens that
	// printing's own card page; there is no price here.
	let { items }: { items: OtherPrinting[] } = $props();
</script>

<ul class="others">
	{#each items as o (o.href)}
		<li>
			<a href={o.href}>{o.set_code.toUpperCase()} #{o.number} · {o.variant} ({o.finishes.join(', ')})</a>
		</li>
	{/each}
</ul>

<style>
	.others {
		list-style: none;
		padding: var(--space-0);
		margin: var(--space-0);
		max-width: 480px;
	}
	li {
		padding: var(--space-2) var(--space-0);
		border-bottom: 1px solid var(--color-border);
	}
	a {
		color: var(--color-text);
		text-decoration: none;
	}
	a:hover {
		color: var(--color-text-accent);
	}
</style>
