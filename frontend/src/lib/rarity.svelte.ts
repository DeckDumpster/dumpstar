// The official rarity marks (one-letter character, colour, sort value), read
// once from /api/rarities. The catalog apply stores them from the official
// rarity record; nothing about a rarity's look is decided here.

import { api } from './api';
import type { RarityMark } from './types/RarityMark';

class RarityMarks {
	map = $state<Record<string, RarityMark>>({});
	loaded = $state(false);
	private loading: Promise<void> | null = null;

	async load(): Promise<void> {
		if (this.loaded) return;
		if (this.loading) return this.loading;
		this.loading = (async () => {
			try {
				const list = await api.rarities();
				const m: Record<string, RarityMark> = {};
				for (const r of list) m[r.name.toLowerCase()] = r;
				this.map = m;
				this.loaded = true;
			} finally {
				this.loading = null;
			}
		})();
		return this.loading;
	}

	get(name: string | null | undefined): RarityMark | undefined {
		return name ? this.map[name.toLowerCase()] : undefined;
	}
}

export const rarityMarks = new RarityMarks();
