// An expired or missing Pocket ID session answers every `/api` call with a 401
// (the server cannot send `fetch` to a cross-origin sign-in page). The page
// answers by taking the browser to sign-in and back to where it was; it never
// shows the 401 as an error.

/** The sign-in URL that returns to `path` (path plus query and hash) afterwards. */
export function signInUrl(path: string): string {
	return `/auth/login?return_to=${encodeURIComponent(path)}`;
}

/**
 * `fetch`, except a 401 navigates to sign-in, returning to the current page, and
 * never settles: the page is leaving, so no caller renders an error first.
 */
export async function sessionFetch(input: RequestInfo | URL, init?: RequestInit): Promise<Response> {
	const res = await fetch(input, init);
	if (res.status === 401 && typeof window !== 'undefined') {
		const { pathname, search, hash } = window.location;
		window.location.assign(signInUrl(pathname + search + hash));
		return new Promise<Response>(() => {});
	}
	return res;
}
