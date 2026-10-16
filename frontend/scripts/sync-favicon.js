// Derives the favicon set from the header's own mark, src/lib/components/DeathStar.svelte,
// so the tab icon cannot drift from the brand art: static/favicon.svg carries the
// component's shapes verbatim, and the PNG fallbacks (32x32, and the 180x180
// apple-touch-icon) are rasterised from those same shapes. The mark is square (24x24);
// were it not, it is centred on the square canvas, never stretched.
//
//   node scripts/sync-favicon.js            write static/
//   (tests/favicon.test.js imports `build` and fails when static/ is stale)
import { readFileSync, writeFileSync } from 'node:fs';
import { deflateSync } from 'node:zlib';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
export const SOURCE = join(root, 'src/lib/components/DeathStar.svelte');
export const GOLD = '#f5c542'; // theme.css --pd-gold-500, which is the header's --color-accent

const attrs = (tag) => Object.fromEntries([...tag.matchAll(/([\w-]+)="([^"]*)"/g)].map((m) => [m[1], m[2]]));

export function shapes(svelte = readFileSync(SOURCE, 'utf8')) {
	const svg = svelte.match(/<svg[\s\S]*?<\/svg>/)[0];
	const viewBox = attrs(svg.match(/<svg[^>]*>/)[0]).viewBox.split(/\s+/).map(Number);
	const els = [...svg.matchAll(/<(path|circle|line)\b[^>]*\/>/g)].map((m) => ({ tag: m[1], ...attrs(m[0]), src: m[0] }));
	return { viewBox, els };
}

export function faviconSvg(s = shapes()) {
	const [x, y, w, h] = s.viewBox;
	const side = Math.max(w, h);
	const vb = `${x - (side - w) / 2} ${y - (side - h) / 2} ${side} ${side}`;
	const body = s.els.map((e) => '\t' + e.src.replace(/\sfill="none"/, '')).join('\n');
	return `<svg xmlns="http://www.w3.org/2000/svg" width="${side}" height="${side}" viewBox="${vb}" fill="none" stroke="${GOLD}">
	<title>Dumpstar</title>
${body}
</svg>
`;
}

// Flatten each shape to polylines (in viewBox units) with a stroke width.
function polylines(s) {
	const out = [];
	for (const e of s.els) {
		const sw = Number(e['stroke-width'] ?? 1);
		if (e.tag === 'line') out.push({ sw, pts: [[+e.x1, +e.y1], [+e.x2, +e.y2]] });
		else if (e.tag === 'circle') {
			const pts = [];
			for (let i = 0; i <= 64; i++) pts.push([+e.cx + +e.r * Math.cos((i / 64) * 2 * Math.PI), +e.cy + +e.r * Math.sin((i / 64) * 2 * Math.PI)]);
			out.push({ sw, pts });
		} else {
			const toks = e.d.match(/[MLA]|-?[\d.]+/g);
			let i = 0, cur = null, pts = null;
			const num = () => +toks[i++];
			while (i < toks.length) {
				const c = toks[i++];
				if (c === 'M') { if (pts) out.push({ sw, pts }); cur = [num(), num()]; pts = [cur]; }
				else if (c === 'L') { while (i < toks.length && !/[MLA]/.test(toks[i])) { cur = [num(), num()]; pts.push(cur); } }
				else if (c === 'A') {
					const [rx, , , large, sweep] = [num(), num(), num(), num(), num()];
					const to = [num(), num()];
					pts.push(...arc(cur, to, rx, large, sweep));
					cur = to;
				}
			}
			if (pts) out.push({ sw, pts });
		}
	}
	return out;
}

// SVG endpoint-parameterised circular arc, sampled.
function arc(p, q, r, large, sweep) {
	const mx = (p[0] + q[0]) / 2, my = (p[1] + q[1]) / 2;
	const dx = (p[0] - q[0]) / 2, dy = (p[1] - q[1]) / 2;
	const d2 = dx * dx + dy * dy;
	let k = Math.sqrt(Math.max(0, (r * r - d2) / d2));
	if (large === sweep) k = -k;
	const cx = mx + k * dy, cy = my - k * dx;
	const a0 = Math.atan2(p[1] - cy, p[0] - cx);
	let da = Math.atan2(q[1] - cy, q[0] - cx) - a0;
	if (sweep && da < 0) da += 2 * Math.PI;
	if (!sweep && da > 0) da -= 2 * Math.PI;
	const n = 96, pts = [];
	for (let i = 1; i <= n; i++) pts.push([cx + r * Math.cos(a0 + (da * i) / n), cy + r * Math.sin(a0 + (da * i) / n)]);
	return pts;
}

const segDist = (px, py, a, b) => {
	const vx = b[0] - a[0], vy = b[1] - a[1];
	const t = Math.max(0, Math.min(1, ((px - a[0]) * vx + (py - a[1]) * vy) / (vx * vx + vy * vy || 1)));
	return Math.hypot(px - a[0] - t * vx, py - a[1] - t * vy);
};

const crcTable = Array.from({ length: 256 }, (_, n) => { let c = n; for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1; return c >>> 0; });
const crc = (buf) => { let c = 0xffffffff; for (const b of buf) c = crcTable[(c ^ b) & 255] ^ (c >>> 8); return (c ^ 0xffffffff) >>> 0; };
const chunk = (type, data) => {
	const t = Buffer.concat([Buffer.from(type), data]);
	const len = Buffer.alloc(4); len.writeUInt32BE(data.length);
	const sum = Buffer.alloc(4); sum.writeUInt32BE(crc(t));
	return Buffer.concat([len, t, sum]);
};

// Rasterise to an RGBA PNG: transparent ground, gold round-capped strokes, 4x4 supersampling.
export function png(size, s = shapes()) {
	const [vx, vy, vw, vh] = s.viewBox;
	const side = Math.max(vw, vh), ox = vx - (side - vw) / 2, oy = vy - (side - vh) / 2;
	const scale = size / side, lines = polylines(s);
	const rgb = [0xf5, 0xc5, 0x42], SS = 4;
	const raw = Buffer.alloc((size * 4 + 1) * size);
	for (let y = 0; y < size; y++) {
		raw[y * (size * 4 + 1)] = 0;
		for (let x = 0; x < size; x++) {
			let hit = 0;
			for (let sy = 0; sy < SS; sy++) for (let sx = 0; sx < SS; sx++) {
				const px = ox + (x + (sx + 0.5) / SS) / scale, py = oy + (y + (sy + 0.5) / SS) / scale;
				outer: for (const l of lines) for (let i = 1; i < l.pts.length; i++)
					if (segDist(px, py, l.pts[i - 1], l.pts[i]) <= l.sw / 2) { hit++; break outer; }
			}
			const o = y * (size * 4 + 1) + 1 + x * 4;
			raw[o] = rgb[0]; raw[o + 1] = rgb[1]; raw[o + 2] = rgb[2]; raw[o + 3] = Math.round((hit / (SS * SS)) * 255);
		}
	}
	const ihdr = Buffer.alloc(13);
	ihdr.writeUInt32BE(size, 0); ihdr.writeUInt32BE(size, 4); ihdr[8] = 8; ihdr[9] = 6;
	return Buffer.concat([Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]), chunk('IHDR', ihdr), chunk('IDAT', deflateSync(raw, { level: 9 })), chunk('IEND', Buffer.alloc(0))]);
}

export function build() {
	const s = shapes();
	return {
		'favicon.svg': faviconSvg(s),
		'favicon-32.png': png(32, s),
		'apple-touch-icon.png': png(180, s)
	};
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
	for (const [name, data] of Object.entries(build())) writeFileSync(join(root, 'static', name), data);
}
