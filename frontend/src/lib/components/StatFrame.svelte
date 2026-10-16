<script lang="ts">
	/** A card's Power or HP in the frame the printed card gives it: power in a
	 * red, notched blade-shaped plate; hit points in a blue shield. */
	let { kind, value }: { kind: 'power' | 'hp'; value: number | null | undefined } = $props();
	const label = $derived(kind === 'power' ? 'Power' : 'HP');
</script>

{#if value != null}
	<span class="stat-frame {kind}" role="img" aria-label="{label} {value}" title="{label} {value}">
		<svg viewBox="0 0 24 26" aria-hidden="true" focusable="false">
			{#if kind === 'power'}
				<path class="plate" d="M3 2h18l-2 6 2 6-3 9-6 3-6-3-3-9 2-6-2-6z" />
				<path class="rim" d="M5 4h14l-1.4 4.2L19 14l-2.4 7.4L12 23.2 7.4 21.4 5 14l1.4-5.8z" />
			{:else}
				<path class="plate" d="M2 3l10-2 10 2v10c0 6-5 10-10 12C7 23 2 19 2 13z" />
				<path class="rim" d="M4.2 4.8L12 3.2l7.8 1.6V13c0 4.6-3.8 7.8-7.8 9.4C8 20.8 4.2 17.6 4.2 13z" />
			{/if}
		</svg>
		<span class="n" aria-hidden="true">{value}</span>
	</span>
{/if}

<style>
	.stat-frame {
		position: relative;
		display: inline-grid;
		place-items: center;
		width: 1.7em;
		height: 1.85em;
		vertical-align: middle;
		font-weight: 800;
		line-height: 1;
		font-variant-numeric: tabular-nums;
	}
	svg {
		position: absolute;
		inset: 0;
		width: 100%;
		height: 100%;
	}
	.n {
		position: relative;
		color: var(--color-on-danger);
	}
	.power .plate {
		fill: var(--color-danger-surface);
	}
	.power .rim {
		fill: var(--color-danger);
	}
	.hp .plate {
		fill: var(--color-info-surface);
	}
	.hp .rim {
		fill: var(--color-info);
	}
</style>
