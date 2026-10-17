/**
 * Teach Node's module loader to import `.svelte` files.
 *
 * The primitives in `src/lib/components/ui/` need render tests, and the test
 * runner is Node's built-in one — no jsdom, no testing-library, no extra
 * dependency (the same rule `tokens.contrast.test.js` set). So instead of a
 * DOM we compile each component for the server and render it to a string:
 * markup and the scoped stylesheet are both assertable that way, which is
 * exactly what "renders, respects variants, emits no hardcoded colour" needs.
 *
 * Loaded via `node --import ./tests/support/svelte-hooks.js` (see package.json).
 * Compilation is `generate: 'server'` + `css: 'injected'`, so the component's
 * own CSS travels with it into the rendered `head` rather than being dropped.
 *
 * `<script lang="ts">` needs no preprocessor: Svelte 5 strips type annotations
 * itself.
 */

import { registerHooks } from 'node:module';
import { readFileSync } from 'node:fs';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { dirname, join } from 'node:path';
import { compile } from 'svelte/compiler';
import ts from 'typescript';

const isSvelte = (/** @type {string} */ specifier) => specifier.endsWith('.svelte');

/** `$lib/…` — SvelteKit's own alias, resolved here the way the app resolves
    it. Without it a component that imports a shared helper (`$lib/format`)
    compiles fine and then fails to LOAD under the test runner, which would
    quietly push primitives towards re-implementing what the app already has. */
const libDir = join(dirname(fileURLToPath(import.meta.url)), '../../src/lib');
const isLib = (/** @type {string} */ specifier) =>
	specifier === '$lib' || specifier.startsWith('$lib/');

registerHooks({
	resolve(specifier, context, nextResolve) {
		if (isLib(specifier)) {
			const rest = specifier.slice('$lib'.length).replace(/^\//, '');
			const base = pathToFileURL(join(libDir, rest)).href;
			// A bare `$lib/format` is `format.ts`; an extension is only spelled
			// out for `.svelte` (and for the `.svelte.ts` rune stores).
			const url = /\.(ts|js|svelte)$/.test(rest) ? base : `${base}.ts`;
			if (isSvelte(url)) return { url, format: 'module', shortCircuit: true };
			return nextResolve(url, context);
		}
		if (isSvelte(specifier) && context.parentURL) {
			return {
				url: new URL(specifier, context.parentURL).href,
				format: 'module',
				shortCircuit: true
			};
		}
		return nextResolve(specifier, context);
	},

	load(url, context, nextLoad) {
		// Not every Node 22.x build ships type stripping, and once hooks are
		// registered the default loader refuses `.ts`; transpile it here with
		// the repo's own `typescript`.
		if (url.startsWith('file:') && url.endsWith('.ts') && !url.includes('/node_modules/')) {
			const { outputText } = ts.transpileModule(readFileSync(fileURLToPath(url), 'utf8'), {
				compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2022 }
			});
			return { format: 'module', source: outputText, shortCircuit: true };
		}
		if (!isSvelte(url)) return nextLoad(url, context);
		const filename = fileURLToPath(url);
		const { js } = compile(readFileSync(filename, 'utf8'), {
			filename,
			generate: 'server',
			css: 'injected'
		});
		return { format: 'module', source: js.code, shortCircuit: true };
	}
});
