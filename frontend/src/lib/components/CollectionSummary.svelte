<!--
	The "Your collection" body of the front page: the valued figures when a
	valuation snapshot covers the collection, an owned-count note when cards are
	owned but not yet valued (the snapshot is written by the daily valuation, so
	cards added since the last run have no row), and the empty state only when
	nothing is owned.
-->
<script lang="ts">
	import Badge from '$lib/kit/ui/Badge.svelte';
	import Button from '$lib/kit/ui/Button.svelte';
	import EmptyState from '$lib/kit/ui/EmptyState.svelte';
	import Panel from '$lib/kit/ui/Panel.svelte';

	let {
		loading,
		valued,
		ownedCards,
		valuedCards,
		ownedLabel,
		ownedMeta,
		valueMeta,
		marketLabel,
		costLabel,
		unrealized,
		unrealizedLabel
	}: {
		loading: boolean;
		valued: boolean;
		ownedCards: number;
		valuedCards: string;
		ownedLabel: string;
		ownedMeta: string;
		valueMeta: string;
		marketLabel: string;
		costLabel: string;
		unrealized: number | null;
		unrealizedLabel: string;
	} = $props();
</script>

{#if valued}
	<div class="stats">
		<Panel padding="md">
			<div class="stat">
				<span class="stat-label">Cards owned</span>
				<strong class="stat-figure">{valuedCards}</strong>
				<span class="stat-meta">{ownedMeta}</span>
			</div>
		</Panel>
		<Panel padding="md">
			<div class="stat">
				<span class="stat-label">Market value</span>
				<strong class="stat-figure">{marketLabel}</strong>
				<span class="stat-meta">{valueMeta}</span>
			</div>
		</Panel>
		<Panel padding="md">
			<div class="stat">
				<span class="stat-label">Cost basis</span>
				<strong class="stat-figure">{costLabel}</strong>
				<span class="stat-meta">what you paid</span>
			</div>
		</Panel>
		<Panel padding="md">
			<div class="stat">
				<span class="stat-label">Unrealised</span>
				<span class="stat-figure">
					{#if unrealized != null}
						<Badge tone={unrealized < 0 ? 'danger' : 'success'}>
							{unrealized < 0 ? '−' : '+'}{unrealizedLabel}
						</Badge>
					{/if}
				</span>
				<span class="stat-meta">market less cost</span>
			</div>
		</Panel>
	</div>
{:else if ownedCards > 0}
	<Panel padding="md">
		<div class="stat">
			<span class="stat-label">Cards owned</span>
			<strong class="stat-figure">{ownedLabel}</strong>
			<span class="stat-meta">
				Not valued yet — market value and cost basis appear after the next valuation, which runs
				daily at 07:30 UTC.
			</span>
		</div>
	</Panel>
{:else if !loading}
	<Panel padding="md">
		<EmptyState
			title="No cards registered yet"
			description="Import a CSV export from another tracker, or open a set as a binder page and click the cards you own."
		>
			{#snippet action()}
				<Button href="/ingest/csv">Import a CSV</Button>
			{/snippet}
		</EmptyState>
	</Panel>
{/if}

<style>
	/* --- Headline figures --------------------------------------------------
	   Four equal columns, never auto-fit: half the complaint about this page
	   was that its tiles took whatever width the track algorithm handed them. */
	.stats {
		display: grid;
		grid-template-columns: repeat(4, minmax(0, 1fr));
		gap: var(--space-3);
	}
	.stat {
		display: flex;
		flex-direction: column;
		gap: var(--space-1);
	}
	.stat-label {
		font-size: var(--text-sm);
		text-transform: uppercase;
		letter-spacing: 0.06em;
		color: var(--color-text-subtle);
	}
	/* Fixed line box so the Badge in the "Unrealised" tile sits on the same
	   baseline as the three plain figures beside it. */
	.stat-figure {
		display: flex;
		align-items: center;
		min-height: calc(var(--text-2xl) * var(--leading-tight));
		font-size: var(--text-2xl);
		font-weight: var(--weight-semibold);
		line-height: var(--leading-tight);
		color: var(--color-text-strong);
	}
	.stat-meta {
		font-size: var(--text-md);
		color: var(--color-text-subtle);
	}
	@media (max-width: 1100px) {
		.stats {
			grid-template-columns: repeat(2, minmax(0, 1fr));
		}
	}
</style>
