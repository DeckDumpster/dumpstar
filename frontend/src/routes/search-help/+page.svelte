<script lang="ts">
	import { onMount } from 'svelte';
	import { api } from '$lib/api';
	import { breadcrumbs } from '$lib/breadcrumbs.svelte';
	import type { SearchVocabulary } from '$lib/types/SearchVocabulary';

	breadcrumbs.set([{ label: 'Collection', href: '/collection' }, { label: 'Search Help' }]);

	let vocab = $state<SearchVocabulary | null>(null);
	let error = $state<string | null>(null);

	onMount(async () => {
		try {
			vocab = await api.searchKeywords();
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		}
	});

	// Operators are syntax, not data — documented inline.
	const operators: { op: string; meaning: string }[] = [
		{ op: ':', meaning: 'contains / matches (the default)' },
		{ op: '=', meaning: 'exact match' },
		{ op: '!=', meaning: 'not equal' },
		{ op: '<  >  <=  >=', meaning: 'numeric / ordinal comparison' }
	];

	const examples: { q: string; what: string }[] = [
		{ q: 't:unit a:vigilance cost<=3', what: 'Vigilance units that cost 3 or less, that you own' },
		{ q: 's:sor rarity>=rare', what: 'Spark of Rebellion cards, rare or better' },
		{ q: 'is:missing s:sor t:leader', what: "Spark of Rebellion leaders you're still missing" },
		{ q: 'is:hyperspace is:foil', what: 'Hyperspace foils in your collection' },
		{ q: 'trait:rebel power>=4 arena:ground', what: 'Rebel ground units with 4+ power' },
		{ q: 'o:ambush', what: 'Cards whose rules text mentions Ambush' },
		{ q: 'price>20 added>=2026-01-01', what: 'Recent pickups worth over $20' },
		{ q: 'luke qty>=2', what: 'Luke printings you own 2 or more copies of' },
		{ q: 'a:cunning or a:aggression -is:graded', what: 'Cunning or Aggression, excluding graded copies' },
		{ q: 'order:price direction:desc', what: 'Everything, most valuable first' }
	];

	/** What `order:` accepts — the collection page's sort keys. */
	const sortKeys = [
		'name',
		'number',
		'set',
		'rarity',
		'type',
		'cost',
		'power',
		'hp',
		'price',
		'qty',
		'added'
	];
	const hasValues = ['text', 'subtitle', 'artist'];

	function aliasList(aliases: string[]): string {
		return aliases.join(', ');
	}
</script>

<svelte:head><title>Search Help — Dumpstar</title></svelte:head>

<div class="wrap" data-testid="search-help">
	<h1>Search syntax</h1>
	<p class="lead">
		The collection search bar speaks a Scryfall-style query language: combine keywords with
		<code>AND</code> (just a space), <code>or</code>, <code>-</code> (not), and parentheses. By
		default it searches the cards you own; add <code>is:missing</code> to include cards you don't.
	</p>

	{#if error}
		<p class="error">Couldn't load the keyword list: {error}</p>
	{/if}

	<section>
		<h2>Operators</h2>
		<table>
			<tbody>
				{#each operators as o (o.op)}
					<tr><td class="mono">{o.op}</td><td>{o.meaning}</td></tr>
				{/each}
			</tbody>
		</table>
	</section>

	<section>
		<h2>Examples</h2>
		<table>
			<tbody>
				{#each examples as ex (ex.q)}
					<tr><td class="mono">{ex.q}</td><td>{ex.what}</td></tr>
				{/each}
			</tbody>
		</table>
	</section>

	<section>
		<h2><code>order:</code> and <code>has:</code></h2>
		<table>
			<tbody>
				<tr>
					<td class="mono">order:</td>
					<td>
						Sort key: {#each sortKeys as k, i (k)}{#if i > 0}, {/if}<code>{k}</code>{/each}.
						Add <code>direction:desc</code> to reverse it.
					</td>
				</tr>
				<tr>
					<td class="mono">has:</td>
					<td>
						Presence: {#each hasValues as h, i (h)}{#if i > 0}, {/if}<code>has:{h}</code>{/each}.
					</td>
				</tr>
			</tbody>
		</table>
	</section>

	{#if vocab}
		<section data-testid="search-vocab">
			<h2>Keywords</h2>
			<table>
				<thead><tr><th>Keyword</th><th>Operators</th><th>Description</th></tr></thead>
				<tbody>
					{#each vocab.keywords as k (k.canonical)}
						<tr>
							<td class="mono">{aliasList(k.aliases)}</td>
							<td class="mono ops">{k.operators.join(' ')}</td>
							<td>{k.help ?? ''}</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</section>

		<section>
			<h2><code>is:</code> flags</h2>
			<table>
				<tbody>
					{#each vocab.flags as f (f.flag)}
						<tr><td class="mono">is:{f.flag}</td><td>{f.help ?? ''}</td></tr>
					{/each}
				</tbody>
			</table>
		</section>
	{:else if !error}
		<p class="muted">Loading keywords…</p>
	{/if}
</div>

<style>
	.wrap {
		max-width: 60rem;
		margin: 0 auto;
		padding: 1.5rem 1.25rem 4rem;
	}
	h1 {
		margin: 0 0 0.5rem;
	}
	h2 {
		margin: 2rem 0 0.6rem;
		font-size: 1.1rem;
		color: var(--color-text-muted);
	}
	.lead {
		color: var(--color-text-muted);
		line-height: 1.5;
		max-width: 48rem;
	}
	table {
		width: 100%;
		border-collapse: collapse;
		font-size: 0.92rem;
	}
	th {
		text-align: left;
		color: var(--color-text-subtle);
		font-weight: 600;
		border-bottom: 1px solid var(--color-border-subtle);
		padding: 0.4rem 0.6rem;
	}
	td {
		padding: 0.35rem 0.6rem;
		border-bottom: 1px solid var(--color-border-subtle);
		vertical-align: top;
	}
	.mono {
		font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
		color: var(--color-warning-text);
		white-space: nowrap;
	}
	.ops {
		color: var(--color-info-text);
	}
	code {
		font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
		background: var(--color-surface-raised);
		padding: 0.05rem 0.3rem;
		border-radius: 0.25rem;
		color: var(--color-warning-text);
	}
	.error {
		color: var(--color-danger-text);
	}
	.muted {
		color: var(--color-text-subtle);
	}
</style>
