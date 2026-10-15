import { test } from 'node:test';
import assert from 'node:assert/strict';
import { latestOnly } from '../src/lib/latest.js';

const deferred = () => {
	/** @type {(v?: any) => void} */
	let resolve = () => {};
	/** @type {Promise<any>} */
	const promise = new Promise((r) => (resolve = r));
	return { promise, resolve: (/** @type {any} */ v) => resolve(v) };
};

test('a delayed earlier response cannot overwrite a newer page', async () => {
	const l = latestOnly();
	const slow = deferred();
	const first = l.run('page=1', () => slow.promise);
	const second = l.run('page=3', async () => 'page 3');
	assert.deepEqual(await second, { stale: false, value: 'page 3' });
	slow.resolve('page 1');
	assert.deepEqual(await first, { stale: true });
});

test('a superseded request is aborted', async () => {
	const l = latestOnly();
	/** @type {AbortSignal | undefined} */
	let signal;
	const slow = deferred();
	void l.run('a', (s) => ((signal = s), slow.promise));
	void l.run('b', async () => 1);
	assert.equal(signal?.aborted, true);
});

test('an identical in-flight request is not issued twice', async () => {
	const l = latestOnly();
	let calls = 0;
	const d = deferred();
	const a = l.run('same', () => (calls++, d.promise));
	const b = l.run('same', () => (calls++, d.promise));
	d.resolve(1);
	assert.deepEqual(await a, { stale: false, value: 1 });
	assert.deepEqual(await b, { stale: true });
	assert.equal(calls, 1);
});

test('after completion the same request may run again', async () => {
	const l = latestOnly();
	await l.run('k', async () => 1);
	assert.deepEqual(await l.run('k', async () => 2), { stale: false, value: 2 });
});
