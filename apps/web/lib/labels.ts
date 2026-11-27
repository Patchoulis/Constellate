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
	/** may be left out: if every place it could go covers a label already placed, it is hidden and takes no room */
	optional?: boolean;
}

export interface Placed {
	left: number;
	top: number;
	/** an optional label that found no free place: not to be shown */
	hidden?: true;
}

const EDGE = 6; // px a label keeps from the frame's edges
const STEPS = 5; // how many label-heights away from its point a label may be moved
const SPACE = 4; // px between two labels

export function placeLabels(frame: { width: number; height: number }, labels: LabelBox[]): Placed[] {
	const taken: (Placed & { width: number; height: number })[] = [];
	const within = (start: number, size: number, room: number) => Math.max(EDGE, Math.min(start, room - size - EDGE));
	return labels.map(({ x, y, width, height, gap, above, optional }) => {
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
		if (optional && least > 0) return { ...at, hidden: true };
		taken.push({ ...at, width, height });
		return at;
	});
}

/** A paper somewhere on screen (px inside the graph), by its index in the graph. */
export interface PaperPlace {
	index: number;
	x: number;
	y: number;
}

/** A marked paper's place and the places of the papers it is linked to. */
export interface MarkedPlace extends PaperPlace {
	linked: PaperPlace[];
}

/**
 * Which marked papers, and which of the papers they are linked to, get a label: those inside the frame, each
 * once, none of `skip` (they are labelled already), at most `most.marked` marked papers (newest first) and
 * `most.linked` linked papers. The linked papers are taken in turn from each marked paper, its closest first, so
 * every marked paper gets some of its neighbours named before any gets all of them.
 */
export function pickMarkedLabels(
	frame: { width: number; height: number },
	marked: MarkedPlace[],
	skip: ReadonlySet<number>,
	most: { marked: number; linked: number },
): { marked: PaperPlace[]; linked: PaperPlace[] } {
	const inside = ({ x, y }: PaperPlace) => x >= 0 && y >= 0 && x <= frame.width && y <= frame.height;
	const taken = new Set(skip);
	// a linked paper that is marked itself is named as a marked paper, or not at all
	for (const { index } of marked) taken.add(index);
	const take = (place: PaperPlace) => {
		if (!inside(place) || taken.has(place.index)) return false;
		taken.add(place.index);
		return true;
	};
	const pickedMarked = marked.filter((place) => inside(place) && !skip.has(place.index)).slice(0, most.marked);
	// only the newest marked papers name their neighbours, so no neighbour is named for a mark left out by the cap
	// (one that is off screen or hovered still does: its links are drawn)
	const sources = marked.slice(0, most.marked);
	const linked: PaperPlace[] = [];
	const longest = Math.max(0, ...sources.map((place) => place.linked.length));
	for (let rank = 0; rank < longest && linked.length < most.linked; rank++)
		for (const place of sources) {
			const other = place.linked[rank];
			if (other && take(other)) linked.push(other);
			if (linked.length === most.linked) break;
		}
	return { marked: pickedMarked, linked };
}
