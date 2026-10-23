/**
 * How the deck page lays out a deck. The deck holds real copies (one row per
 * copy); the page shows lines: copies of the same printing, finish and
 * condition collapse to one line with a quantity. The line carries which copy
 * it is (printing, finish, condition) in its link and data, never as text.
 */
import type { CollectionRow } from '$lib/types/CollectionRow';

export type DeckLine = {
	key: string;
	name: string;
	qty: number;
	cost: number | null;
	aspects: string[];
	image: string | null;
	/** The deployed side of a leader printing. */
	back: string | null;
	type: string | null;
	setCode: string;
	number: string;
	href: string;
	printingId: string;
	finish: string;
	condition: string;
	copyIds: number[];
};

export type DeckGroup = { label: string; count: number; lines: DeckLine[] };

export type DeckView = {
	leaders: DeckLine[];
	bases: DeckLine[];
	groups: DeckGroup[];
	/** Cards (not copies of leaders/bases) by cost; the last bucket is `CURVE_MAX`+. */
	curve: number[];
	aspectTotals: { name: string; count: number }[];
	total: number;
};

export const CURVE_MAX = 7;

function list(json: string | null): string[] {
	if (!json) return [];
	try {
		const v = JSON.parse(json);
		return Array.isArray(v) ? v.filter((x) => typeof x === 'string') : [];
	} catch {
		return [];
	}
}

/** The card page for the exact printing, finish and condition a copy is. */
export function copyHref(c: Pick<CollectionRow, 'set_code' | 'number' | 'finish' | 'condition'>): string {
	const q = new URLSearchParams({ finish: c.finish, condition: c.condition });
	return `/card/${encodeURIComponent(c.set_code)}/${encodeURIComponent(c.number)}?${q}`;
}

const GROUPS = ['Ground Units', 'Space Units', 'Units', 'Events', 'Upgrades', 'Tokens', 'Other'];

function groupLabel(c: CollectionRow): string {
	switch (c.card_type) {
		case 'Unit': {
			const arenas = list(c.arenas);
			if (arenas.includes('Space')) return 'Space Units';
			if (arenas.includes('Ground')) return 'Ground Units';
			return 'Units';
		}
		case 'Event':
			return 'Events';
		case 'Upgrade':
			return 'Upgrades';
		case 'Token':
			return 'Tokens';
		default:
			return 'Other';
	}
}

export function buildDeckView(cards: CollectionRow[]): DeckView {
	const lines = new Map<string, DeckLine & { arenas: string }>();
	for (const c of cards) {
		const key = `${c.printing_id}|${c.finish}|${c.condition}`;
		const have = lines.get(key);
		if (have) {
			have.qty += 1;
			have.copyIds.push(c.id);
			continue;
		}
		lines.set(key, {
			key,
			name: c.name,
			qty: 1,
			cost: c.cost,
			aspects: list(c.aspects),
			image: c.image_url,
			back: c.back_image_url,
			setCode: c.set_code,
			number: c.number,
			href: copyHref(c),
			printingId: c.printing_id,
			finish: c.finish,
			condition: c.condition,
			copyIds: [c.id],
			type: c.card_type,
			arenas: groupLabel(c)
		});
	}
	const sorted = [...lines.values()].sort(
		(a, b) => (a.cost ?? 99) - (b.cost ?? 99) || a.name.localeCompare(b.name)
	);
	const view: DeckView = {
		leaders: [],
		bases: [],
		groups: [],
		curve: new Array(CURVE_MAX + 1).fill(0),
		aspectTotals: [],
		total: 0
	};
	const byGroup = new Map<string, DeckLine[]>();
	const aspects = new Map<string, number>();
	for (const l of sorted) {
		if (l.type === 'Leader') {
			view.leaders.push(l);
			continue;
		}
		if (l.type === 'Base') {
			view.bases.push(l);
			continue;
		}
		const g = byGroup.get(l.arenas) ?? [];
		g.push(l);
		byGroup.set(l.arenas, g);
		view.total += l.qty;
		if (l.cost != null && l.type !== 'Token') view.curve[Math.min(l.cost, CURVE_MAX)] += l.qty;
		for (const a of l.aspects) aspects.set(a, (aspects.get(a) ?? 0) + l.qty);
	}
	for (const label of GROUPS) {
		const ls = byGroup.get(label);
		if (ls) view.groups.push({ label, count: ls.reduce((n, l) => n + l.qty, 0), lines: ls });
	}
	view.aspectTotals = [...aspects].map(([name, count]) => ({ name, count }));
	return view;
}
