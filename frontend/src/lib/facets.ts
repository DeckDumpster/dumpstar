// Shared builder for "facet" links — clickable card metadata (artist, set,
// rarity, type, aspect, trait, …) that lands on the collection
// page pre-filtered by the proper search DSL. Used by the card detail/modal
// (CardDetailView) and the collection table view so both emit identical,
// correct queries instead of a bare card-name search.
//
// The DSL keywords live in the backend search compiler
// (crates/dumpstar-db/src/search.rs); this module only formats them. A value is
// quoted only when the lexer would otherwise split it — it tokenizes an
// unquoted keyword value up to whitespace or ')', so ':', '-', '*', '.' are
// safe bare (see crates/dumpstar-core/src/query/lexer.rs::read_value).

/** Format a single `field:value` DSL clause, quoting the value when needed.
 *  Pass `field = ''` for a bare term (an implicit name-contains search). */
export function facetClause(field: string, value: string): string {
	const needsQuote = /[\s()]/.test(value);
	// Card metadata never contains a literal `"`; strip defensively so a stray
	// one can't terminate the quoted run early.
	const v = needsQuote ? `"${value.replace(/"/g, '')}"` : value;
	return field ? `${field}:${v}` : v;
}

/** The query the collection page is showing. A facet link is built far from
 *  the page (card modal, table cells), so the page publishes its query here
 *  and `facetHref` reads it. Only `is:unowned` is carried from it. */
let currentQuery = '';
export function setFacetContext(q: string): void {
	currentQuery = q;
}

const UNOWNED = /(^|[\s(])is:unowned(?=$|[\s)])/i;

/** A `/collection` URL pre-filtered by one facet. The query alone decides the
 *  results: a facet click is an owned-only search, unless the query being left
 *  already says `is:unowned`, in which case the new query keeps that
 *  predicate so a catalog-wide exploration stays catalog-wide. */
export function facetHref(field: string, value: string, from: string = currentQuery): string {
	const clause = facetClause(field, value);
	const q = UNOWNED.test(from) ? `is:unowned ${clause}` : clause;
	return `/collection?${new URLSearchParams({ q }).toString()}`;
}
