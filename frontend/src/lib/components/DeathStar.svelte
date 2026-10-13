<script lang="ts">
	// The Dumpstar mark — the second Death Star, red strokes on transparent, DD-style.
	// One source of truth for every place that renders the logo
	// (breadcrumb chrome, /collection brand, home hero).
	let {
		size = 22,
		stroke = 'var(--color-accent)'
	}: { size?: number; stroke?: string } = $props();
</script>

<!-- The stroke rides in on a custom property and is painted by the CSS `stroke`
     property rather than the presentation attribute: a `var()` in an attribute
     value is not reliably resolved, and the default has to be a token. -->
<svg
	viewBox="0 0 24 24"
	width={size}
	height={size}
	xmlns="http://www.w3.org/2000/svg"
	aria-hidden="true"
	style:--deathstar-stroke={stroke}
>
	<path d="M2.15 13.74 A10 10 0 1 1 10.26 21.85" fill="none" stroke-width="2" />
	<path d="M2.15 13.74 L5.5 15 L7 13.2 M10.26 21.85 L9 18.5 L6.5 17.5 L5.5 15" fill="none" stroke-width="1.5" />
	<circle cx="15.5" cy="8.2" r="2.4" fill="none" stroke-width="1.75" />
	<line x1="8" y1="12" x2="22" y2="12" stroke-width="1.75" />
</svg>

<style>
	path,
	circle,
	line {
		stroke: var(--deathstar-stroke);
	}
</style>
