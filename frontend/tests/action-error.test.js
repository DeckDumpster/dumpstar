import { test } from "node:test";
import assert from "node:assert/strict";
import { api } from "../src/lib/api.ts";
import { createActionErrors } from "../src/lib/actionError.js";
import ActionErrorOverlay from "../src/lib/components/ActionErrorOverlay.svelte";
import { markup } from "./support/render.js";

/** Stub fetch with the given statuses in order; returns the call log. */
/** @param {...number} statuses */
function stubFetch(...statuses) {
  /** @type {any[]} */
  const calls = [];
  globalThis.fetch = async (url, init) => {
    calls.push([init?.method ?? "GET", url]);
    const status = statuses.shift() ?? 200;
    return new Response(status === 200 ? "{}" : "gateway", {
      status,
      statusText: "x",
    });
  };
  return calls;
}

/** Drive an add-a-copy through the controller the way the browse page does. */
/** @param {any} ctl @param {string} name */
async function addCopy(ctl, name) {
  const send = () => api.addCopy({ printing_id: "p1", source: "binder_click" });
  const run = async () => {
    try {
      await send();
    } catch (e) {
      ctl.fail("add:p1", `add ${name}`, e, run);
    }
  };
  await run();
}

test("a 502 on add-a-copy shows the transient overlay; Try again sends exactly one request", async () => {
  const calls = stubFetch(502, 200);
  /** @type {any[]} */
  let items = [];
  const ctl = createActionErrors((i) => (items = i));
  await addCopy(ctl, "Sabine Wren");
  assert.equal(calls.length, 1, "no automatic retry");
  assert.equal(items.length, 1);
  assert.equal(items[0].transient, true);
  const html = markup(ActionErrorOverlay, { items, ondismiss() {} });
  assert.match(html, /Couldn&#39;t add Sabine Wren|Couldn't add Sabine Wren/);
  assert.match(html, /briefly unavailable/);
  assert.match(html, /Try again/);
  const headline = /<strong>(.*?)<\/strong>/.exec(html)?.[1] ?? "";
  assert.doesNotMatch(headline, /\/api|502/);
  await items[0].onretry();
  assert.equal(calls.length, 2, "Try again re-sends exactly one request");
  assert.equal(items.length, 0, "success clears the message");
});

test("a 404 names the action, hides the path behind details, offers no retry", async () => {
  stubFetch(404);
  /** @type {any[]} */
  let items = [];
  const ctl = createActionErrors((i) => (items = i));
  await addCopy(ctl, "Sabine Wren");
  assert.equal(items[0].transient, false);
  const html = markup(ActionErrorOverlay, { items, ondismiss() {} });
  assert.match(html, /<details>/);
  assert.match(html, /404/);
  assert.doesNotMatch(html, /Try again/);
  const headline = /<strong>(.*?)<\/strong>/.exec(html)?.[1] ?? "";
  assert.doesNotMatch(headline, /\/api|404/);
});

test("network errors are transient; one message per action", async () => {
  globalThis.fetch = async () => {
    throw new TypeError("fetch failed");
  };
  /** @type {any[]} */
  let items = [];
  const ctl = createActionErrors((i) => (items = i));
  await addCopy(ctl, "A");
  await addCopy(ctl, "A");
  assert.equal(items.length, 1);
  assert.equal(items[0].transient, true);
  ctl.dismiss("add:p1");
  assert.equal(items.length, 0);
});
