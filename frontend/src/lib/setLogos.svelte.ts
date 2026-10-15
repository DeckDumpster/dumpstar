import { api } from '$lib/api';

// Set code -> the official logo served from /art, read once from /api/sets.
// A set with no retrieved logo is simply absent, and its code stands in.
let logos = $state<Record<string, string>>({});
let started = false;

export function loadSetLogos() {
	if (started) return;
	started = true;
	api
		.sets()
		.then((sets) => {
			const next: Record<string, string> = {};
			for (const s of sets) if (s.logo_url) next[s.set_code.toLowerCase()] = s.logo_url;
			logos = next;
		})
		.catch(() => {
			started = false;
		});
}

export function setLogo(code: string): string | undefined {
	return logos[code.toLowerCase()];
}
