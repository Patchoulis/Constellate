/**
 * Changes to the settings, saved one after another: which save is the latest asked for, the settings the
 * engine answered with, and a change it refused, with its reason, until the user saves that setting again
 * or puts the message away.
 *
 *     asked --saved--> the answer is the settings (only the latest save's: an older one says less)
 *     asked --refused--> the refusal is kept; the settings are read again from the engine --read--> shown,
 *                        unless another save was asked for meanwhile (its answer will say more)
 *
 * Pure, so that it is tested (tests/shell/settings-saves.test.ts); app/page.tsx holds it.
 */

export interface Refusal<T> {
	/** what the user asked for, which was not saved */
	changes: Partial<T>;
	/** the engine's reason, in its own words */
	error: string;
}

export interface Saves<T> {
	/** the number of the latest save asked for */
	asked: number;
	/** the settings the latest save answered with; undefined: the engine's, as read */
	saved?: T;
	refused: Refusal<T> | null;
}

export type SaveEvent<T> =
	| { kind: "asked" }
	| { kind: "saved"; id: number; changes: Partial<T>; settings: T }
	| { kind: "refused"; id: number; changes: Partial<T>; error: string }
	/** the settings read from the engine after the save numbered `id` was refused */
	| { kind: "read"; id: number; settings: T }
	| { kind: "dismissed" };

export const NO_SAVES = { asked: 0, refused: null } as const;

export function saveStep<T>(state: Saves<T>, event: SaveEvent<T>): Saves<T> {
	switch (event.kind) {
		case "asked":
			return { ...state, asked: state.asked + 1 };
		case "saved": {
			// a refusal stays until the setting it was about is saved after all
			const settled = state.refused && Object.keys(state.refused.changes).every((key) => key in event.changes);
			return {
				...state,
				saved: event.id === state.asked ? event.settings : state.saved,
				refused: settled ? null : state.refused,
			};
		}
		case "refused":
			// what is shown stays until the engine's settings are read again (`read`)
			return { ...state, refused: { changes: event.changes, error: event.error } };
		case "read":
			return event.id === state.asked ? { ...state, saved: event.settings } : state;
		case "dismissed":
			return { ...state, refused: null };
	}
}
