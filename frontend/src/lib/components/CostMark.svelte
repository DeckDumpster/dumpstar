<script lang="ts">
	let { cost }: { cost: number | null | undefined } = $props();
</script>

{#if cost != null}
	<span class="cost-mark" role="img" aria-label={`Cost ${cost}`} title={`Cost ${cost}`}>
		<svg viewBox="0 0 24 26" aria-hidden="true" focusable="false">
			<path d="M12 1 22.5 7v12L12 25 1.5 19V7z" fill="var(--cost-mark-fill)" stroke="var(--cost-mark-edge)" stroke-width="1.6" stroke-linejoin="round" />
			<text font-size="14" x="12" y="13.5" text-anchor="middle" dominant-baseline="central" fill="var(--cost-mark-text)">{cost}</text>
		</svg>
	</span>
{/if}

<style>
	.cost-mark {
		display: inline-flex;
		vertical-align: middle;
	}
	.cost-mark svg {
		width: 1.5em;
		height: 1.6em;
	}
	.cost-mark text {
		font-weight: 800;
		font-family: inherit;
	}
</style>
