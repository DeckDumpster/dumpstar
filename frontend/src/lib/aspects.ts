// The six Star Wars: Unlimited aspects. The icons are hot-linked from the
// publisher's CDN (Ryan's verdict on sp-eakf89: the media kit licenses use to
// stores and forbids redistribution), so the repository keeps no copy of them
// and nothing here is served by the app. `base` is the CDN file name without
// its size prefix (media kit, chapter 8).
const CDN = 'https://cdn.starwarsunlimited.com//';

export interface Aspect {
	name: string;
	base: string;
	colour: string;
	sort_value: number;
}

/** Aspects in the official order. */
export const ASPECTS: Aspect[] = [
	{ name: 'Vigilance', base: 'SWH_Aspects_Vigilance_0519102eb9', colour: '#4073d4', sort_value: 1 },
	{ name: 'Command', base: 'SWH_Aspects_Command_79e2808348', colour: '#0b992d', sort_value: 5 },
	{ name: 'Aggression', base: 'SWH_Aspects_Aggression_ceca8f7d7c', colour: '#d30808', sort_value: 10 },
	{ name: 'Cunning', base: 'SWH_Aspects_Cunning_91fedef0ce', colour: '#eb9f1c', sort_value: 15 },
	{ name: 'Villainy', base: 'SWH_Aspects_Villainy_3e06e5ffdb', colour: '#000000', sort_value: 20 },
	{ name: 'Heroism', base: 'SWH_Aspects_Heroism_fd98140fb6', colour: '#ffffff', sort_value: 25 }
];

const BY_NAME = new Map(ASPECTS.map((a) => [a.name.toLowerCase(), a]));

/** The row for an aspect name, or undefined for a name the set lacks. */
export function aspectOf(name: string): Aspect | undefined {
	return BY_NAME.get(name.toLowerCase());
}

/** The smallest official rendition, for `src`. */
export function aspectIconUrl(a: Aspect): string {
	return `${CDN}xxxsmall_${a.base}.png`;
}

/** The kit's three sizes as a `srcset`. */
export function aspectIconSrcset(a: Aspect): string {
	return [
		`${CDN}xxxsmall_${a.base}.png 149w`,
		`${CDN}xxsmall_${a.base}.png 199w`,
		`${CDN}thumbnail_${a.base}.png 248w`
	].join(', ');
}
