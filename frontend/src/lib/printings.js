// Plain JS so the node test runner loads it as is.
/** @typedef {import('./types/PrintingInfo').PrintingInfo} PrintingInfo */
/** @typedef {import('./types/PriceSeries').PriceSeries} PriceSeries */
/** @typedef {{ set_code: string, number: string, href: string, variant: string, finishes: string[] }} OtherPrinting */

/** The card page's own route for a printing: `/card/{set}/{number}`.
 * @param {{ set_code: string, number: string }} p */
export function printingHref(p) {
	return `/card/${encodeURIComponent(p.set_code)}/${encodeURIComponent(p.number)}`;
}

/** Normal before Foil; catalog printings before user-added ones.
 * @param {PrintingInfo} a @param {PrintingInfo} b */
function finishOrder(a, b) {
	return (
		Number(a.is_user_added) - Number(b.is_user_added) ||
		Number(a.finish === 'Foil') - Number(b.finish === 'Foil')
	);
}

/** The printings that live at this page's set + collector number: Normal and
 *  Foil of one variant, and any user-added variant. These are switched on the
 *  page itself.
 * @param {PrintingInfo[]} printings @param {string} setCode @param {string} number */
export function siblingsAt(printings, setCode, number) {
	return printings
		.filter((p) => p.set_code === setCode && p.number === number)
		.sort(finishOrder);
}

/** The one printing the page is about: the picked one if it is still here,
 *  else Normal before Foil.
 * @param {PrintingInfo[]} printings @param {string} setCode @param {string} number
 * @param {string | null} pickedId
 * @returns {PrintingInfo | null} */
export function shownPrinting(printings, setCode, number, pickedId) {
	const here = siblingsAt(printings, setCode, number);
	return here.find((p) => p.printing_id === pickedId) ?? here[0] ?? null;
}

/** Printings of the same card at a different set or collector number, one
 *  entry per place, each pointing at that printing's own page. No prices.
 *  Keeps the server's order (set, number, variant, finish).
 * @param {PrintingInfo[]} printings @param {string} setCode @param {string} number
 * @returns {OtherPrinting[]} */
export function otherPrintings(printings, setCode, number) {
	/** @type {Map<string, OtherPrinting>} */
	const byPlace = new Map();
	for (const p of printings) {
		if (p.is_user_added) continue;
		if (p.set_code === setCode && p.number === number) continue;
		const key = `${p.set_code}\u0000${p.number}`;
		const seen = byPlace.get(key);
		if (seen) {
			if (!seen.finishes.includes(p.finish)) seen.finishes.push(p.finish);
		} else {
			byPlace.set(key, {
				set_code: p.set_code,
				number: p.number,
				href: printingHref(p),
				variant: p.variant,
				finishes: [p.finish]
			});
		}
	}
	return [...byPlace.values()];
}

/** The price series of the shown printing alone.
 * @param {PriceSeries[]} series @param {string | null} printingId */
export function seriesFor(series, printingId) {
	return printingId ? series.filter((s) => s.printing_id === printingId) : [];
}
