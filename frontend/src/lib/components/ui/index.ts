/**
 * The UI primitives live in dumpster-kit, vendored at the revision kit.lock
 * pins (src/lib/kit/ui). Routes keep importing from here:
 *
 *     import { Panel, Button, Field } from '$lib/components/ui';
 *
 * A variant a route needs is added in the kit and picked up by a pin bump,
 * never patched here.
 */
export * from '$lib/kit/ui';
