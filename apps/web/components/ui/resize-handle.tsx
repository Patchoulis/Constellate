"use client";

import { useEffect, useRef, useState } from "react";
import { draggedSize, keyedSize, type Edge } from "@/lib/ui/panel-sizes";
import { cn } from "@/lib/utils";

export interface ResizeHandleProps {
	label: string;
	edge: Edge;
	/** the panel's size now, and the sizes it can be given */
	value: number;
	min: number;
	max: number;
	/** show a size while dragging, without committing it */
	onPreview: (size: number) => void;
	onCommit: (size: number) => void;
	/** back to the panel's initial size */
	onReset: () => void;
	className?: string;
}

interface Drag {
	pointer: number;
	start: number;
	from: number;
	size: number;
	frame: number;
	/** laid over the whole window while dragging: frames and embedded pages would otherwise take the pointer */
	cover: HTMLDivElement;
}

/**
 * The edge of a resizable panel: drag it, or focus it and use the arrow keys (Shift for larger steps), Home and
 * End; double-click it or press Enter for the initial size. It lies over the panel's edge, taking no room from it.
 * Its panel must be positioned (`relative`, `absolute`).
 */
export function ResizeHandle({ label, edge, value, min, max, onPreview, onCommit, onReset, className }: ResizeHandleProps) {
	const across = edge === "top"; // a handle along the top edge changes a height
	const cursor = across ? "row-resize" : "col-resize";
	const drag = useRef<Drag | null>(null);
	const [dragging, setDragging] = useState(false);

	const stop = () => {
		const now = drag.current;
		if (!now) return null;
		cancelAnimationFrame(now.frame);
		now.cover.remove();
		document.body.style.userSelect = "";
		drag.current = null;
		setDragging(false);
		return now;
	};
	// a panel closed in the middle of a drag (Escape closes the reader) leaves nothing over the window
	useEffect(() => () => void stop(), []);

	return (
		<div
			role="separator"
			tabIndex={0}
			aria-label={label}
			aria-orientation={across ? "horizontal" : "vertical"}
			aria-valuenow={value}
			aria-valuemin={min}
			aria-valuemax={max}
			data-dragging={dragging || undefined}
			className={cn(
				"group/resize absolute z-30 touch-none outline-none",
				across ? "inset-x-0 -top-[3px] h-1.5 cursor-row-resize" : "inset-y-0 w-1.5 cursor-col-resize",
				edge === "left" && "-left-[3px]",
				edge === "right" && "-right-[3px]",
				className,
			)}
			onPointerDown={(event) => {
				if (event.button !== 0 || drag.current) return; // one drag at a time, whatever other pointers do
				event.preventDefault(); // no text selection starts here
				event.currentTarget.setPointerCapture(event.pointerId);
				const cover = document.createElement("div");
				cover.style.cssText = `position:fixed;inset:0;z-index:2147483647;cursor:${cursor}`;
				document.body.appendChild(cover);
				document.body.style.userSelect = "none";
				const from = across ? event.clientY : event.clientX;
				drag.current = { pointer: event.pointerId, start: value, from, size: value, frame: 0, cover };
				setDragging(true);
			}}
			onPointerMove={(event) => {
				const now = drag.current;
				if (!now || event.pointerId !== now.pointer) return;
				now.size = draggedSize(now.start, now.from, across ? event.clientY : event.clientX, edge, { min, max });
				now.frame ||= requestAnimationFrame(() => {
					now.frame = 0;
					onPreview(now.size);
				});
			}}
			// the end of every drag: the button let go, the pointer cancelled, the capture taken away
			onLostPointerCapture={(event) => {
				if (event.pointerId !== drag.current?.pointer) return;
				const ended = stop();
				if (!ended) return;
				onPreview(ended.size); // a frame cancelled by stop() may have left a size from partway through
				if (ended.size !== ended.start) onCommit(ended.size);
			}}
			onDoubleClick={onReset}
			onKeyDown={(event) => {
				const next = keyedSize(event.key, event.shiftKey, value, edge, { min, max });
				if (next === null) return;
				event.preventDefault();
				if (next === "reset") onReset();
				else onCommit(next);
			}}
		>
			<span
				className={cn(
					"pointer-events-none absolute bg-transparent transition-colors delay-75 group-hover/resize:bg-ring group-focus-visible/resize:bg-ring group-data-dragging/resize:bg-ring",
					across ? "inset-x-0 top-1/2 h-0.5 -translate-y-1/2" : "inset-y-0 left-1/2 w-0.5 -translate-x-1/2",
				)}
			/>
		</div>
	);
}
