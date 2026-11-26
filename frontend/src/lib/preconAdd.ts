import type { Deck } from './types/Deck';

export type AddOutcome = { deck: Deck } | { refusal: string };

/**
 * Add a preconstructed deck. An unconfirmed repeat is answered 409 with the
 * server's reason, returned as `refusal` for the page to put to the user; every
 * other failure is thrown.
 */
export async function tryAddPrecon(
	add: (id: string, confirmRepeat: boolean) => Promise<Deck>,
	id: string,
	confirmRepeat: boolean
): Promise<AddOutcome> {
	try {
		return { deck: await add(id, confirmRepeat) };
	} catch (e) {
		const { status, body } = e as { status?: number; body?: string };
		if (confirmRepeat || status !== 409 || !body) throw e;
		return { refusal: body.replace(/^conflict: /, '') };
	}
}
