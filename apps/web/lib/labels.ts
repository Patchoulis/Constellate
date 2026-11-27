/**
 * Where to put labels that each belong to a point, so that every one stays inside the frame and, as far as there
 * is room, none covers another.
 *
 * Labels are placed in the order given (the most important first). Each tries the four corners around its point,
 * then the same corners a step further away, and takes the first place that is free; if nothing is free, the
 * place that covers the least of what is already there.
 */
export interface LabelBox {
	/** the point the label belongs to */
	x: number;
	y: number;
	/** the label's size */
	width: number;
	height: number;
	/** px between the point and the label */
	gap: number;
	/** try above the point before below it */
	above?: boolean;
}

export interface Placed {
	left: number;
	top: number;
}

const EDGE = 6; // px a label keeps from the frame's edges
const STEPS = 5; // how many label-heights away from its point a label may be moved
const SPACE = 4; // px between two labels

export function placeLabels(frame: { width: number; height: number }, labels: LabelBox[]): Placed[] {
	const taken: (Placed & { width: number; height: number })[] = [];
	const within = (start: number, size: number, room: number) => Math.max(EDGE, Math.min(start, room - size - EDGE));
	return labels.map(({ x, y, width, height, gap, above }) => {
		const covered = (at: Placed) =>
			taken.reduce((sum, other) => {
				const across = Math.min(at.left + width, other.left + other.width + SPACE) - Math.max(at.left, other.left - SPACE);
				const down = Math.min(at.top + height, other.top + other.height + SPACE) - Math.max(at.top, other.top - SPACE);
				return sum + Math.max(0, across) * Math.max(0, down);
			}, 0);
		const sides = above ? [-1, 1] : [1, -1];
		let best: Placed | undefined;
		let least = Infinity;
		search: for (let step = 0; step < STEPS; step++) {
			for (const vertical of sides) {
				for (const horizontal of [1, -1]) {
					const away = gap + step * (height + SPACE);
					const at = {
						left: within(horizontal > 0 ? x + gap : x - gap - width, width, frame.width),
						top: within(vertical > 0 ? y + away : y - away - height, height, frame.height),
					};
					const area = covered(at);
					if (area < least) {
						best = at;
						least = area;
						if (area === 0) break search;
					}
				}
			}
		}
		const at = best ?? { left: EDGE, top: EDGE };
		taken.push({ ...at, width, height });
		return at;
	});
}
