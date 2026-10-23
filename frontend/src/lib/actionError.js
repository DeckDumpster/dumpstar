/**
 * Turning an API failure into something a person can read.
 *
 * A request that fails while the site restarts (network drop, 502/503/504,
 * Cloudflare's 52x origin-down pages) is *transient*: the action did not go
 * through and sending it again is reasonable. Everything else (4xx, 500) is a
 * plain failure that names the action; the raw path and status stay in
 * `details`. Nothing here retries on its own: a lost add-a-copy response may
 * already have been applied, so the user re-sends it knowingly.
 */

/** @param {unknown} err */
export function isTransient(err) {
  const status = /** @type {any} */ (err)?.status;
  if (typeof status === "number") {
    return (
      status === 502 ||
      status === 503 ||
      status === 504 ||
      (status >= 520 && status <= 530)
    );
  }
  // fetch() rejects with a TypeError when the connection fails outright.
  return err instanceof TypeError;
}

/**
 * @param {unknown} err
 * @param {string} action what failed, as a verb phrase: "add Sabine Wren"
 */
export function describeFailure(err, action) {
  const transient = isTransient(err);
  return {
    transient,
    title: transient
      ? `Couldn't ${action} — the site is briefly unavailable`
      : `Couldn't ${action}`,
    body: transient
      ? "That did not go through. The site may be restarting; give it a moment, then try again."
      : "Something went wrong on our end.",
    details: err instanceof Error ? err.message : String(err),
  };
}

/**
 * One message per action key: a second failure of the same action replaces
 * the first. `retry` re-sends the action once, on the user's click.
 *
 * @param {(items: any[]) => void} onChange
 */
export function createActionErrors(onChange) {
  /** @type {Map<string, any>} */
  const items = new Map();
  const emit = () => onChange([...items.values()]);
  return {
    /**
     * @param {string} key one per action ("add:<printing>")
     * @param {string} action
     * @param {unknown} err
     * @param {(() => Promise<unknown>) | null} resend
     */
    fail(key, action, err, resend) {
      const f = describeFailure(err, action);
      items.set(key, {
        key,
        ...f,
        onretry: f.transient && resend ? () => this.retry(key) : null,
        resend,
      });
      emit();
    },
    /** @param {string} key */
    dismiss(key) {
      if (items.delete(key)) emit();
    },
    /** @param {string} key */
    async retry(key) {
      const it = items.get(key);
      if (!it?.resend) return;
      items.delete(key);
      emit();
      await it.resend();
    },
    clear() {
      items.clear();
      emit();
    },
  };
}
