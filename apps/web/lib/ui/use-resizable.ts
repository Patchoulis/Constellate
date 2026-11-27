"use client";

import { useCallback, useMemo, useRef, useSyncExternalStore, type CSSProperties } from "react";
import type { ResizeHandleProps } from "@/components/ui/resize-handle";
import {
	fitSizes,
	initialSizes,
	limitsOf,
	PANEL_SIZES_KEY,
	PANELS,
	parsePanelSizes,
	serialisePanelSizes,
	type Frame,
	type PanelId,
	type Sizes,
} from "@/lib/ui/panel-sizes";

/** The sizes the user chose, the window they are shown in, and what that makes of them. */
interface Layout {
	wanted: Sizes;
	frame: Frame | null;
	fitted: Sizes;
}

const layoutOf = (wanted: Sizes, frame: Frame | null): Layout => ({ wanted, frame, fitted: fitSizes(wanted, frame) });

/** Where the page is rendered without a window: the initial sizes. The first render in the window agrees with it. */
const SERVER = layoutOf(initialSizes(), null);

/**
 * One layout for the whole window, shared by every panel: a side panel's room depends on the other side panel's
 * size. Read from storage the first time it is asked for in the window.
 */
let current: Layout | null = null;
const listeners = new Set<() => void>();

const frameNow = (): Frame => ({ width: window.innerWidth, height: window.innerHeight });

function load(): Sizes {
	try {
		return parsePanelSizes(window.localStorage.getItem(PANEL_SIZES_KEY));
	} catch (error) {
		return initialSizes(); // storage cannot be read (a private window, a disabled store)
	}
}

function save(sizes: Sizes) {
	try {
		window.localStorage.setItem(PANEL_SIZES_KEY, serialisePanelSizes(sizes));
	} catch (error) {
		// storage is full or switched off: the sizes still hold until the window is closed
	}
}

function snapshot(): Layout {
	return (current ??= layoutOf(load(), frameNow()));
}

function publish(next: Layout) {
	current = next;
	for (const listener of listeners) listener();
}

/** Follow the window's size, at most once a frame: panels give way when it gets smaller and come back after. */
let pending = 0;
function onWindowResize() {
	if (pending) return;
	pending = requestAnimationFrame(() => {
		pending = 0;
		const { wanted, frame } = snapshot();
		const now = frameNow();
		if (frame?.width !== now.width || frame?.height !== now.height) publish(layoutOf(wanted, now));
	});
}

function subscribe(listener: () => void) {
	if (listeners.size === 0) {
		// the window may have changed while nothing was listening; React reads the snapshot again after this
		const { wanted } = snapshot();
		current = layoutOf(wanted, frameNow());
		window.addEventListener("resize", onWindowResize);
	}
	listeners.add(listener);
	return () => {
		listeners.delete(listener);
		if (listeners.size === 0) window.removeEventListener("resize", onWindowResize);
	};
}

function choose(id: PanelId, size: number) {
	const { wanted, frame } = snapshot();
	if (wanted[id] === size) return;
	const next = { ...wanted, [id]: size };
	publish(layoutOf(next, frame));
	save(next);
}

/** A panel's size as shown now, for code that measures with it outside rendering (centring a paper beside the reader). */
export function panelSize(id: PanelId): number {
	return (typeof window === "undefined" ? SERVER : snapshot()).fitted[id];
}

/** The CSS variable a resizable panel's size is in; the panel sizes itself with it (`w-(--panel-size)`). */
const SIZE_VAR = "--panel-size";

/**
 * One resizable panel. Put `ref` and `style` on the element whose size changes (its width or height is
 * `var(--panel-size)`), and `<ResizeHandle {...handle} />` inside the panel, on the edge facing the graph.
 *
 * A drag moves only the CSS variable on that element, once a frame, without rendering anything; the size is
 * committed (rendered, remembered) when the drag ends.
 */
export function useResizable<T extends HTMLElement = HTMLElement>(id: PanelId) {
	const layout = useSyncExternalStore(subscribe, snapshot, () => SERVER);
	const ref = useRef<T>(null);
	const size = layout.fitted[id];
	const { min, max } = limitsOf(id, layout.fitted, layout.frame);
	const onPreview = useCallback((next: number) => ref.current?.style.setProperty(SIZE_VAR, `${next}px`), []);
	// The element is set to what is shown at once: where rendering would not change the style (the size shown
	// stays what it was before the drag), a size left there by the drag would otherwise stay.
	const onCommit = useCallback(
		(next: number) => {
			choose(id, next);
			onPreview(snapshot().fitted[id]);
		},
		[id, onPreview],
	);
	const onReset = useCallback(() => onCommit(PANELS[id].initial), [id, onCommit]);
	const style = useMemo(() => ({ [SIZE_VAR]: `${size}px` }) as CSSProperties, [size]);
	const { label, edge } = PANELS[id];
	const handle: ResizeHandleProps = { label, edge, value: size, min, max, onPreview, onCommit, onReset };
	return { ref, style, size, handle };
}
