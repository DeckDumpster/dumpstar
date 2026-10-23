<script lang="ts">
	import AspectIcon from './AspectIcon.svelte';
	import CostMark from './CostMark.svelte';
	import { CURVE_MAX, type DeckView, type DeckLine } from '$lib/deckView';

	let {
		view,
		busy = false,
		onRemove
	}: { view: DeckView; busy?: boolean; onRemove?: (copyId: number) => void } = $props();

	let preview = $state<DeckLine | null>(null);
	const peak = $derived(Math.max(1, ...view.curve));

	function show(l: DeckLine) {
		preview = l;
	}
	function tap(e: MouseEvent, l: DeckLine) {
		if ((e.target as HTMLElement).closest('a, button')) return;
		preview = preview?.key === l.key ? null : l;
	}
</script>

{#snippet row(l: DeckLine)}
			<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
			<li
				class="line"
				data-printing={l.printingId}
				data-finish={l.finish}
				data-condition={l.condition}
				onmouseenter={() => show(l)}
				onfocusin={() => show(l)}
				onclick={(e) => tap(e, l)}
			>
				<span class="qty">{l.qty}×</span>
				<CostMark cost={l.cost} />
				<a class="name" href={l.href}>{l.name}</a>
				<span class="asp">
					{#each l.aspects as a (a)}<AspectIcon name={a} size="1.1em" />{/each}
				</span>
				{#if onRemove}
					<button
						type="button"
						class="rm"
						disabled={busy}
						aria-label="Remove one {l.name}"
						onclick={() => onRemove(l.copyIds[0])}>−</button
					>
				{/if}
			</li>
{/snippet}

<div class="deck">
	<aside class="pinned" aria-label="Leader">
		{#each view.leaders as l (l.key)}
			<figure class="pin" data-printing={l.printingId} data-condition={l.condition}>
				<a href={l.href}>
					{#if l.image}<img src={l.image} alt={l.name} />{:else}<span class="noart">{l.name}</span>{/if}
				</a>
				<figcaption>{l.name}</figcaption>
			</figure>
		{/each}
		<div class="preview" aria-hidden="true">
			{#if preview?.image}<img src={preview.image} alt="" />{/if}
		</div>
	</aside>

	<div class="lists">
		<section class="summary" aria-label="Deck summary">
			<div class="curve" role="img" aria-label="Cost curve">
				{#each view.curve as n, cost (cost)}
					<div class="col">
						<span class="n">{n || ''}</span>
						<span class="bar" style:height="{(n / peak) * 100}%"></span>
						<span class="c">{cost === CURVE_MAX ? `${CURVE_MAX}+` : cost}</span>
					</div>
				{/each}
			</div>
			<ul class="aspects">
				{#each view.aspectTotals as a (a.name)}
					<li><AspectIcon name={a.name} /> {a.count}</li>
				{/each}
			</ul>
		</section>

		<div class="groups">
		{#if view.leaders.length || view.bases.length}
			<section class="group" aria-label="Leader and base">
				<h2>Leader and base <span class="count">{view.leaders.length + view.bases.length}</span></h2>
				<ul>
					{#each [...view.leaders, ...view.bases] as l (l.key)}
						{@render row(l)}
					{/each}
				</ul>
			</section>
		{/if}

		{#each view.groups as g (g.label)}
			<section class="group">
				<h2>{g.label} <span class="count">{g.count}</span></h2>
				<ul>
					{#each g.lines as l (l.key)}
					{@render row(l)}
					{/each}
				</ul>
			</section>
		{/each}
		</div>
	</div>
</div>

<style>
	.deck {
		display: grid;
		grid-template-columns: minmax(0, 20rem) minmax(0, 1fr);
		gap: var(--space-4);
		margin-top: var(--space-4);
		align-items: start;
	}
	.pinned {
		position: sticky;
		top: var(--space-2);
		display: flex;
		flex-direction: column;
		gap: var(--space-2);
	}
	.pin {
		margin: 0;
	}
	.pin img,
	.preview img {
		display: block;
		width: 100%;
		height: auto;
		border-radius: var(--radius-xl);
	}
	.pin figcaption {
		font-size: var(--text-sm);
		color: var(--color-text-subtle);
	}
	.noart {
		display: block;
		padding: var(--space-4);
		background: var(--color-surface-panel);
		border-radius: var(--radius-xl);
	}
	.preview {
		width: 60%;
		align-self: center;
	}
	.lists {
		min-width: 0;
	}
	.summary {
		display: flex;
		flex-wrap: wrap;
		gap: var(--space-4);
		align-items: flex-end;
		margin-bottom: var(--space-3);
	}
	.curve {
		display: flex;
		gap: var(--space-1);
		height: 6rem;
		align-items: flex-end;
	}
	.col {
		display: flex;
		flex-direction: column;
		justify-content: flex-end;
		align-items: center;
		height: 100%;
		width: 1.6rem;
		font-size: var(--text-xs);
	}
	.bar {
		display: block;
		width: 100%;
		background: var(--color-accent);
		border-radius: var(--radius-xs) var(--radius-xs) 0 0;
	}
	.aspects {
		display: flex;
		flex-wrap: wrap;
		gap: var(--space-3);
		list-style: none;
		margin: 0;
		padding: 0;
	}
	h2 {
		font-size: var(--text-lg);
		color: var(--color-text-accent);
		border-bottom: 1px solid var(--color-border);
		margin: var(--space-3) 0 var(--space-1);
	}
	.count {
		color: var(--color-text-subtle);
		font-weight: normal;
	}
	ul {
		list-style: none;
		margin: 0;
		padding: 0;
	}
	.line {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		padding: var(--space-1) var(--space-2);
		border-bottom: 1px solid var(--color-border);
		cursor: pointer;
	}
	.line:hover {
		background: var(--color-surface-panel);
	}
	.qty {
		min-width: 2rem;
		color: var(--color-text-subtle);
	}
	.groups {
		column-width: 22rem;
		column-gap: var(--space-4);
	}
	.group {
		break-inside: avoid;
	}
	.name {
		flex: 0 1 auto;
		min-width: 0;
		overflow-wrap: anywhere;
		color: var(--color-text);
	}
	.name:hover {
		color: var(--color-text-accent);
	}
	.asp {
		display: inline-flex;
		gap: var(--space-0-5);
	}
	.rm {
		margin-left: auto;
		background: none;
		border: 1px solid var(--color-border);
		color: var(--color-text-subtle);
		border-radius: var(--radius-sm);
		cursor: pointer;
	}
	@media (max-width: 720px) {
		.deck {
			grid-template-columns: minmax(0, 1fr);
		}
		.pinned {
			position: static;
			flex-direction: row;
			flex-wrap: wrap;
		}
		.pin {
			flex: 1 1 45%;
			min-width: 0;
		}
		.preview {
			position: fixed;
			right: var(--space-2);
			bottom: var(--space-2);
			width: 40vw;
			z-index: 10;
		}
	}
</style>
