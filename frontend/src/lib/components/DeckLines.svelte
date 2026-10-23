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
	let open = $state<DeckLine | null>(null);
	// The modal is loaded on first use, so a deck page that never opens one never pays for it.
	let Modal = $state<typeof import('./CardModal.svelte').default | null>(null);
	$effect(() => {
		if (open && !Modal) import('./CardModal.svelte').then((m) => (Modal = m.default));
	});
	// One slot: the hovered line, else the deck's first leader.
	const shown = $derived(preview ?? view.leaders[0] ?? null);
	const isLeader = $derived(shown?.type === 'Leader');
	const isBase = $derived(shown?.type === 'Base');
	// A leader previews its deployed side; a base is turned so its title reads on the left.
	const shownImage = $derived(shown ? (isLeader ? (shown.back ?? shown.image) : shown.image) : null);
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
				<a
						class="name"
						href={l.href}
						onclick={(e) => {
							if (e.ctrlKey || e.metaKey || e.shiftKey || e.button) return;
							e.preventDefault();
							open = l;
						}}>{l.name}</a
					>
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
	<aside class="pinned" aria-label="Card preview">
		<button
			type="button"
			class="preview"
			data-slot="preview"
			data-card={shown?.printingId}
			data-side={isLeader && shown?.back ? 'back' : 'front'}
			data-rotate={isBase ? 'landscape' : undefined}
			aria-label={shown ? `Open ${shown.name}` : 'No card to preview'}
			disabled={!shown}
			onclick={() => shown && (open = shown)}
		>
			{#if shownImage}
				<img src={shownImage} alt={shown?.name ?? ''} />
			{:else if shown}
				<span class="noart">{shown.name}</span>
			{/if}
		</button>
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
		{#if view.leaders.length}
			<section class="group" aria-label="Leader">
				<h2>Leader <span class="count">{view.leaders.length}</span></h2>
				<ul>
					{#each view.leaders as l (l.key)}
						{@render row(l)}
					{/each}
				</ul>
			</section>
		{/if}
		{#if view.bases.length}
			<section class="group" aria-label="Base">
				<h2>Base <span class="count">{view.bases.length}</span></h2>
				<ul>
					{#each view.bases as l (l.key)}
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

{#if open && Modal}
	<Modal setCode={open.setCode} number={open.number} onClose={() => (open = null)} />
{/if}

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
	.noart {
		display: block;
		padding: var(--space-4);
		background: var(--color-surface-panel);
		border-radius: var(--radius-xl);
	}
	.preview {
		position: relative;
		display: block;
		width: 70%;
		margin: 0 auto;
		padding: 0;
		border: 0;
		background: none;
		cursor: pointer;
	}
	.preview img {
		display: block;
		width: 100%;
		height: auto;
		border-radius: var(--radius-xl);
	}
	/* Bases are printed landscape: turned a quarter turn, title on the left. */
	.preview[data-rotate='landscape'] {
		aspect-ratio: 5 / 7;
	}
	.preview[data-rotate='landscape'] img {
		position: absolute;
		top: 50%;
		left: 50%;
		width: 140%;
		max-width: none;
		transform: translate(-50%, -50%) rotate(-90deg);
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
		.preview {
			position: fixed;
			right: var(--space-2);
			bottom: var(--space-2);
			width: 40vw;
			margin: 0;
			z-index: 10;
		}
	}
</style>
