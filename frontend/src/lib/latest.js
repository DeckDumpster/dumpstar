/**
 * Request ordering for a view that refetches as its inputs change: only the
 * newest `run` may deliver a result, a run identical to the one in flight is
 * dropped, and a superseded run is aborted. `run` resolves to `{ stale: true }`
 * when a newer run took over, so the caller leaves its state alone.
 */
export function latestOnly() {
	let seq = 0;
	let key = '';
	/** @type {AbortController | null} */
	let ctl = null;
	return {
		/**
		 * @template T
		 * @param {string} k identity of the request's inputs
		 * @param {(signal: AbortSignal) => Promise<T>} fetcher
		 * @returns {Promise<{ stale: true } | { stale: false, value: T }>}
		 */
		async run(k, fetcher) {
			if (ctl && k === key) return { stale: true };
			ctl?.abort();
			const mine = (ctl = new AbortController());
			key = k;
			const id = ++seq;
			try {
				const value = await fetcher(mine.signal);
				return id === seq ? { stale: false, value } : { stale: true };
			} catch (e) {
				if (id !== seq) return { stale: true };
				throw e;
			} finally {
				if (id === seq) {
					ctl = null;
					key = '';
				}
			}
		}
	};
}
