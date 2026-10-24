// @ts-nocheck — stubs a minimal window.location
import { test } from "node:test";
import assert from "node:assert/strict";
import { api } from "../src/lib/api.ts";
import { signInUrl } from "../src/lib/session.ts";

/** Run `act` with a 401 stub and a fake deep-page location; resolve with where it went. */
async function with401(act) {
  globalThis.fetch = async () => new Response("", { status: 401 });
  let went = null;
  globalThis.window = {
    location: {
      pathname: "/decks/42",
      search: "?tab=cards",
      hash: "",
      assign: (/** @type {string} */ u) => (went = u),
    },
  };
  try {
    const outcome = await Promise.race([
      act().then(
        () => "settled",
        () => "settled",
      ),
      new Promise((r) => setTimeout(() => r("pending"), 20)),
    ]);
    return { went, outcome };
  } finally {
    delete globalThis.window;
  }
}

test("a 401 on a read goes to sign-in and returns to the page", async () => {
  const { went, outcome } = await with401(() => api.collection());
  assert.equal(went, signInUrl("/decks/42?tab=cards"));
  assert.equal(outcome, "pending");
});

test("a 401 on a write goes to sign-in, never an error", async () => {
  const { went, outcome } = await with401(() =>
    api.addCopy({ printing_id: "p1", source: "binder_click" }),
  );
  assert.equal(went, signInUrl("/decks/42?tab=cards"));
  assert.equal(outcome, "pending");
});

test("a 401 on search goes to sign-in", async () => {
  const { went } = await with401(() => api.collectionSearch("x"));
  assert.equal(went, signInUrl("/decks/42?tab=cards"));
});

test("a genuine 500 is still an error and does not navigate", async () => {
  globalThis.fetch = async () => new Response("", { status: 500, statusText: "x" });
  let went = null;
  globalThis.window = { location: { pathname: "/", search: "", hash: "", assign: (u) => (went = u) } };
  try {
    await assert.rejects(api.collection(), { status: 500 });
    assert.equal(went, null);
  } finally {
    delete globalThis.window;
  }
});
