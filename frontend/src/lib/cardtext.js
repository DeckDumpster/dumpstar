// Card text with the official inline game icons. The catalog stores the
// publisher's styled text verbatim (HTML, icons as <img> tags pointing at the
// publisher's host); `cardTextHtml` reduces it to the icon/strong/em subset
// and points every icon at the repository's own copy — data/remedial/
// inline-icons/, copied to static/icons/ at build (scripts/sync-icons.js).
// Nothing is hot-linked, and an icon the manifest lacks renders as nothing.
import manifest from '../../../data/remedial/inline-icons/manifest.json' with { type: 'json' };

/** @typedef {{ file: string, alt: string }} Icon */

/** @type {Map<string, Icon>} */
const ICONS = new Map(manifest.icons.map((i) => [i.file, i]));

/** Tags kept as they are; everything else is dropped, its text kept. */
const KEPT = new Set(['p', 'strong', 'b', 'em', 'i']);

const ENTITY = /^&(#\d+|#x[0-9a-fA-F]+|[a-zA-Z][a-zA-Z0-9]*);/;

/** @param {string} s */
function escapeText(s) {
	let out = '';
	for (let i = 0; i < s.length; i++) {
		const c = s[i];
		if (c === '&') out += ENTITY.test(s.slice(i)) ? '&' : '&amp;';
		else if (c === '>') out += '&gt;';
		else out += c;
	}
	return out;
}

/** @param {string} s */
function escapeAttr(s) {
	return s.replace(/&/g, '&amp;').replace(/"/g, '&quot;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
}

/** @param {string} attrs @param {string} name @returns {string | null} */
function attr(attrs, name) {
	const m = new RegExp(`(?:^|\\s)${name}\\s*=\\s*(?:"([^"]*)"|'([^']*)')`, 'i').exec(attrs);
	return m ? (m[1] ?? m[2] ?? '') : null;
}

/** The manifest entry for an icon `src`, matched on its file name.
 *  @param {string} src @returns {Icon | undefined} */
export function iconOf(src) {
	const file = src.split(/[?#]/)[0].split('/').pop() ?? '';
	return ICONS.get(file);
}

/** Where the static icon is served. @param {Icon} i */
export function iconUrl(i) {
	return `/icons/${i.file}`;
}

/** Sanitised HTML for the styled text: p/strong/b/em/i, <br>, and icons from
 *  the repository's asset set with alt text. Safe for `{@html}`.
 *  @param {string | null | undefined} styled */
export function cardTextHtml(styled) {
	if (!styled) return '';
	const token = /<!--[\s\S]*?-->|<(\/?)([a-zA-Z][a-zA-Z0-9]*)([^>]*)>|<|[^<]+/g;
	/** @type {string[]} */
	const open = [];
	let out = '';
	for (const m of styled.matchAll(token)) {
		const [whole, slash, rawName, attrs] = m;
		if (rawName === undefined) {
			if (whole.startsWith('<!--')) continue;
			out += whole === '<' ? '&lt;' : escapeText(whole);
			continue;
		}
		const name = rawName.toLowerCase();
		if (name === 'img' && !slash) {
			const icon = iconOf(attr(attrs, 'src') ?? '');
			if (icon) {
				out += `<img class="gameIcon" src="${escapeAttr(iconUrl(icon))}" alt="${escapeAttr(icon.alt)}" width="18" height="18">`;
			}
		} else if (name === 'br') {
			if (!slash) out += '<br>';
		} else if (KEPT.has(name)) {
			if (!slash) {
				open.push(name);
				out += `<${name}>`;
			} else {
				const at = open.lastIndexOf(name);
				if (at >= 0) {
					while (open.length > at) out += `</${open.pop()}>`;
				}
			}
		}
	}
	while (open.length) out += `</${open.pop()}>`;
	return out;
}
