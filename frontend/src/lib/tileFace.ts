/**
 * Which picture a portrait card tile shows. Leaders and bases are printed
 * landscape, which a portrait tile squeezes. A leader shows its deployed unit
 * side, which is portrait; a leader printing with no back, and a base (one
 * sided), show the front turned a quarter turn. Neither picture carries the
 * printed collector number the way a portrait front does, so the caption
 * names the card as well: `#016 Darth Vader`.
 */
export type TileFace = {
	image: string | null;
	rotate: boolean;
	/** True when the picture does not show the collector number. */
	captioned: boolean;
};

export function tileFace(card: {
	card_type?: string | null;
	image_url?: string | null;
	back_image_url?: string | null;
}): TileFace {
	const front = card.image_url ?? null;
	if (card.card_type === 'Leader') {
		return card.back_image_url
			? { image: card.back_image_url, rotate: false, captioned: true }
			: { image: front, rotate: true, captioned: true };
	}
	if (card.card_type === 'Base') return { image: front, rotate: true, captioned: true };
	return { image: front, rotate: false, captioned: false };
}

/** The tile's caption: `#016 Darth Vader` when the picture lacks the number, else `#016`. */
export function tileCaption(number: string, name: string, face: TileFace): string {
	return face.captioned ? `#${number} ${name}` : `#${number}`;
}
