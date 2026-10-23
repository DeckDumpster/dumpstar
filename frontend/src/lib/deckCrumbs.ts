import type { Crumb } from './breadcrumbs.svelte';

/** star › Decks › <name> — the leaf is the deck's or group's name, never its id. */
export function deckCrumbs(name: string): Crumb[] {
	return [{ label: 'Decks', href: '/decks' }, { label: name }];
}
