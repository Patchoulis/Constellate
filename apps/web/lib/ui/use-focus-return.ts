"use client";

import { useEffect, type RefObject } from "react";

/**
 * When a panel laid over the graph closes, the keyboard's focus goes back to what had it when the panel opened
 * (the list entry that opened it), rather than to the start of the page. Only when the focus was in the panel,
 * or nowhere: a panel closed because something else was clicked leaves the focus where that put it.
 */
export function useFocusReturn(panel: RefObject<HTMLElement | null>) {
	useEffect(() => {
		const opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
		const element = panel.current;
		return () => {
			const now = document.activeElement;
			const lost = !now || now === document.body || (element !== null && element.contains(now));
			if (lost && opener?.isConnected && opener !== document.body) opener.focus({ preventScroll: true });
		};
	}, [panel]);
}
