/**
 * The kit is the one implementation of the tokens and ui primitives.
 *
 * `kit.lock` pins a dumpster-kit revision and the sha256 of every file
 * vendored from it into `src/lib/kit/`. Editing a vendored file, adding a
 * stray one, or leaving a local copy of a primitive behind fails here: change
 * the kit, then `scripts/kit-sync.sh <kit checkout>` at the new revision.
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { existsSync, readFileSync, readdirSync, statSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join, relative, sep } from 'node:path';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const kitDir = join(root, 'src/lib/kit');
const lock = readFileSync(join(root, 'kit.lock'), 'utf8').split('\n').filter(Boolean);

/** @param {string} dir @returns {string[]} */
function walk(dir) {
	return readdirSync(dir).flatMap((n) => {
		const p = join(dir, n);
		return statSync(p).isDirectory() ? walk(p) : [p];
	});
}

test('kit.lock pins a full revision', () => {
	assert.match(lock[0], /^revision = [0-9a-f]{40}$/);
});

test('vendored kit files match the lock, byte for byte', () => {
	/** @type {Map<string, string>} */
	const want = new Map();
	for (const line of lock.slice(1)) {
		const m = /^([0-9a-f]{64}) [ *](\.\/.+)$/.exec(line);
		assert.ok(m, `bad kit.lock line: ${line}`);
		want.set(m[2], m[1]);
	}
	/** @type {Map<string, string>} */
	const have = new Map();
	for (const f of walk(kitDir)) {
		have.set('./' + relative(kitDir, f).split(sep).join('/'), createHash('sha256').update(readFileSync(f)).digest('hex'));
	}
	assert.deepEqual([...have].sort(), [...want].sort());
});

test('dumpstar keeps no private copy of a kit primitive or of the token file', () => {
	const ui = join(root, 'src/lib/components/ui');
	const extra = existsSync(ui) ? readdirSync(ui).filter((n) => n !== 'index.ts') : [];
	assert.deepEqual(extra, [], 'primitives live in src/lib/kit/ui');
	assert.ok(!existsSync(join(root, 'src/lib/styles/tokens.css')), 'tokens.css lives in src/lib/kit');
});
