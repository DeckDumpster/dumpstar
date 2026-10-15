<script lang="ts" module>
	// Set code -> the official logo served from /art, read once from /api/sets.
	// A set with no retrieved logo is simply absent, and its code stands in.
	let logos = $state<Record<string, string>>({});
	let started = false;

	function loadSetLogos() {
		if (started) return;
		started = true;
		fetch('/api/sets')
			.then((r) => (r.ok ? r.json() : Promise.reject(new Error(String(r.status)))))
			.then((sets: { set_code: string; logo_url?: string | null }[]) => {
				const next: Record<string, string> = {};
				for (const s of sets) if (s.logo_url) next[s.set_code.toLowerCase()] = s.logo_url;
				logos = next;
			})
			.catch(() => {
				started = false;
			});
	}
</script>

<script lang="ts">
	let { code, name = '' }: { code: string; name?: string } = $props();

	loadSetLogos();
	const src = $derived(logos[code.toLowerCase()]);
</script>

{#if src}
	<img class="setmark" {src} alt="{name || code} logo" loading="lazy" />
{/if}

<style>
	.setmark {
		height: 1.4em;
		width: auto;
		max-width: 4.5em;
		object-fit: contain;
		vertical-align: middle;
		margin-inline-end: var(--space-1);
	}
</style>
