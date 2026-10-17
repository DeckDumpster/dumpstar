<script lang="ts">
	/**
	 * The card picture on the detail page and the card modal. A leader has a
	 * second face, its deployed unit side; when the printing carries one, a
	 * round flip button at the lower right swaps the picture between the two.
	 * Every other card has no back, so no button.
	 */
	let {
		front,
		back = null,
		name,
		flipped = $bindable(false)
	}: {
		front: string | null;
		back?: string | null;
		name: string;
		flipped?: boolean;
	} = $props();

	const src = $derived(flipped && back ? back : front);
</script>

<div class="art">
	{#if src}
		<img {src} alt={flipped && back ? `${name}, deployed side` : name} />
	{:else}
		<div class="noart">No image</div>
	{/if}
	{#if back}
		<button
			type="button"
			class="flip"
			aria-pressed={flipped}
			aria-label={flipped ? 'Show leader side' : 'Show deployed side'}
			title={flipped ? 'Show leader side' : 'Show deployed side'}
			onclick={() => (flipped = !flipped)}
		>
			<svg viewBox="0 0 24 24" width="20" height="20" aria-hidden="true" fill="none"
				stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
				<path d="M21 12a9 9 0 1 1-3-6.7" />
				<path d="M21 3v6h-6" />
			</svg>
		</button>
	{/if}
</div>

<style>
	.art {
		position: relative;
		display: inline-block;
	}
	.art img {
		display: block;
		width: 320px;
		max-width: 80vw;
		border-radius: var(--radius-xl);
	}
	.noart {
		width: 320px;
		height: 446px;
		display: flex;
		align-items: center;
		justify-content: center;
		background: var(--color-surface-panel);
		border-radius: var(--radius-xl);
		color: var(--color-text-subtle);
	}
	.flip {
		position: absolute;
		right: var(--space-2);
		bottom: var(--space-2);
		width: 2.5rem;
		height: 2.5rem;
		display: flex;
		align-items: center;
		justify-content: center;
		border: 0;
		border-radius: var(--radius-round);
		background: var(--color-accent);
		color: var(--color-on-accent);
		cursor: pointer;
	}
	.flip:hover {
		background: var(--color-accent-hover);
	}
	.flip:active {
		background: var(--color-accent-active);
	}
	@media (max-width: 540px) {
		.art img {
			max-width: 100%;
		}
	}
</style>
