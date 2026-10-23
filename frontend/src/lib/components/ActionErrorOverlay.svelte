<!--
	ActionErrorOverlay — what the page says when an action did not go through.

	Floats over the page (fixed, bottom) so it never moves the content under it.
	One row per failed action; the headline names the action, never a path. A
	transient failure offers "Try again"; the raw request and status sit behind
	a "details" disclosure. Built here so the sibling sites can adopt it.
-->
<script lang="ts">
	type Item = {
		key: string;
		title: string;
		body: string;
		transient: boolean;
		details: string;
		onretry: (() => void) | null;
	};
	type Props = { items: Item[]; ondismiss: (key: string) => void };
	let { items, ondismiss }: Props = $props();
</script>

{#if items.length > 0}
	<div class="overlay" role="region" aria-label="Problems">
		{#each items as it (it.key)}
			<div class="msg" class:transient={it.transient} role="alert">
				<div class="text">
					<strong>{it.title}</strong>
					<span>{it.body}</span>
					<details>
						<summary>details</summary>
						<code>{it.details}</code>
					</details>
				</div>
				<div class="actions">
					{#if it.onretry}
						<button type="button" class="retry" onclick={it.onretry}>Try again</button>
					{/if}
					<button type="button" class="close" aria-label="Dismiss" onclick={() => ondismiss(it.key)}
						>×</button
					>
				</div>
			</div>
		{/each}
	</div>
{/if}

<style>
	.overlay {
		position: fixed;
		left: 50%;
		bottom: var(--space-4);
		transform: translateX(-50%);
		z-index: 50;
		display: flex;
		flex-direction: column;
		gap: var(--space-2);
		width: min(32rem, calc(100vw - var(--space-4) * 2));
	}
	.msg {
		display: flex;
		justify-content: space-between;
		gap: var(--space-3);
		padding: var(--space-3);
		border: 1px solid var(--color-border-subtle);
		border-radius: var(--radius-md);
		background: var(--color-surface-raised);
		color: var(--color-text);
		box-shadow: var(--shadow-md);
	}
	.text {
		display: flex;
		flex-direction: column;
		gap: var(--space-1);
		min-width: 0;
	}
	code {
		overflow-wrap: anywhere;
		color: var(--color-text-subtle);
	}
	.actions {
		display: flex;
		align-items: flex-start;
		gap: var(--space-2);
	}
	button {
		cursor: pointer;
		font: inherit;
		color: inherit;
		background: transparent;
		border: 1px solid var(--color-border-subtle);
		border-radius: var(--radius-sm);
		padding: var(--space-1) var(--space-2);
	}
</style>
