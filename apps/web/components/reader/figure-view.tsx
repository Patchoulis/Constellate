"use client";

import { useEffect, useRef, useState } from "react";
import { FileText } from "lucide-react";
import type { PaperFigure } from "@rp/contracts";
import { Button } from "@/components/ui/button";
import { DRAW_TIME, FIGURE_FRAME, isTold, type Ask, type Told } from "@/lib/reader/figure-frame";
import { cropFor, shownSize, type Crop } from "@/lib/reader/figures";
import { cn } from "@/lib/utils";

/** What a drawing came to. */
type Drawing = { state: "waiting" } | { state: "drawing" } | { state: "drawn"; url: string } | { state: "failed"; why: string };

/**
 * One paper's figures drawn from its PDF, one at a time, in a sandboxed frame (lib/reader/figure-frame.ts): made
 * when the first figure is asked for, and dropped when the paper is closed or a drawing takes longer than
 * DRAW_TIME (the next figure makes a new one).
 */
export class FigureRenderer {
	private frame: HTMLIFrameElement | null = null;
	private opened: Promise<void> | null = null;
	private listener: ((event: MessageEvent) => void) | null = null;
	private waiting = new Map<number | "open" | "ready", { resolve: (told: Told) => void; reject: (error: Error) => void }>();
	private queue: Promise<unknown> = Promise.resolve();
	/** Why the PDF could not be opened, once it could not: no figure of the paper is tried again. */
	private failure: Error | null = null;
	private next = 1;

	constructor(private readonly pdfUrl: string) {}

	/** Draw `crop` of page `page`: a PNG. */
	draw(page: number, crop: Crop): Promise<Blob> {
		const drawn = this.queue.then(() => this.drawNow(page, crop));
		this.queue = drawn.catch(() => undefined);
		return drawn;
	}

	dispose(): void {
		if (this.listener) window.removeEventListener("message", this.listener);
		this.frame?.remove();
		for (const { reject } of this.waiting.values()) reject(new Error("the figure was not drawn"));
		this.waiting.clear();
		this.frame = null;
		this.opened = null;
		this.listener = null;
	}

	private told(key: number | "open" | "ready"): Promise<Told> {
		return new Promise((resolve, reject) => this.waiting.set(key, { resolve, reject }));
	}

	private open(): Promise<void> {
		if (this.failure) return Promise.reject(this.failure);
		if (this.opened) return this.opened;
		const frame = document.createElement("iframe");
		// scripts only: an opaque origin with nothing of the app's (no cookie, no storage, no page to reach)
		frame.setAttribute("sandbox", "allow-scripts");
		frame.setAttribute("aria-hidden", "true");
		frame.tabIndex = -1;
		frame.style.cssText = "position:absolute;width:0;height:0;border:0;visibility:hidden";
		this.listener = (event: MessageEvent) => {
			if (event.source !== frame.contentWindow || !isTold(event.data)) return;
			const told = event.data;
			const key = told.kind === "ready" ? "ready" : told.kind === "opened" ? "open" : (told.id ?? "open");
			const waiter = this.waiting.get(key);
			this.waiting.delete(key);
			if (told.kind === "failed") waiter?.reject(new Error(told.why));
			else waiter?.resolve(told);
		};
		window.addEventListener("message", this.listener);
		const ready = this.told("ready");
		frame.src = FIGURE_FRAME;
		document.body.append(frame);
		this.frame = frame;
		this.opened = (async () => {
			const response = await fetch(this.pdfUrl);
			if (!response.ok)
				throw new Error(response.status === 404 ? "the paper's PDF is not kept here" : `the PDF could not be had (${response.status})`);
			const pdf = await response.arrayBuffer();
			await withTime(ready, "the figure reader did not start");
			const opened = this.told("open");
			this.send({ kind: "open", pdf }, [pdf]);
			await withTime(opened, "the PDF took too long to open");
		})();
		this.opened.catch((error: unknown) => {
			this.failure = error instanceof Error ? error : new Error(String(error));
			this.dispose();
		});
		return this.opened;
	}

	private async drawNow(page: number, crop: Crop): Promise<Blob> {
		await this.open();
		const id = this.next++;
		const drawn = this.told(id);
		this.send({ kind: "draw", id, page, ...crop });
		try {
			const told = await withTime(drawn, "the figure took too long to draw");
			if (told.kind !== "drawn") throw new Error("the figure was not drawn");
			return told.png;
		} catch (error) {
			if ((error as Error).message.includes("too long")) this.dispose();
			throw error;
		}
	}

	private send(ask: Ask, transfer: Transferable[] = []): void {
		// the frame's origin is opaque, so there is no origin to name: it is reached through its own window only
		this.frame?.contentWindow?.postMessage(ask, "*", transfer);
	}
}

function withTime<T>(promise: Promise<T>, why: string): Promise<T> {
	return new Promise((resolve, reject) => {
		const timer = setTimeout(() => reject(new Error(why)), DRAW_TIME);
		promise.then(
			(value) => {
				clearTimeout(timer);
				resolve(value);
			},
			(error: unknown) => {
				clearTimeout(timer);
				reject(error instanceof Error ? error : new Error(String(error)));
			},
		);
	});
}

/** The pixels a figure is drawn with across: enough for the column and for the larger view a click opens. */
const DRAWN_WIDTH = 1600;

/**
 * A figure of the paper as the PDF draws it, drawn when it first scrolls into view and kept while the paper is
 * open; while it is drawn, a quiet box of its own shape, so that the text does not jump. A click shows it larger,
 * within the reader. When it cannot be drawn, a line says why and the caption stands alone.
 */
export function FigureView({
	figure,
	renderer,
	caption,
	inside,
	onOpenPdf,
}: {
	figure: PaperFigure;
	renderer: FigureRenderer | null;
	/** its caption, as the text shows captions */
	caption: React.ReactNode;
	/** the passages of text drawn inside it */
	inside: string[];
	onOpenPdf?: (page: number) => void;
}) {
	const [drawing, setDrawing] = useState<Drawing>({ state: "waiting" });
	const [large, setLarge] = useState(false);
	const box = useRef<HTMLDivElement | null>(null);
	const size = shownSize(figure);

	useEffect(() => {
		const element = box.current;
		if (!element || !renderer || drawing.state !== "waiting") return;
		const seen = new IntersectionObserver(
			(entries) => {
				if (!entries.some((entry) => entry.isIntersecting)) return;
				seen.disconnect();
				const crop = cropFor(figure, DRAWN_WIDTH);
				if (!crop) return setDrawing({ state: "failed", why: "the figure has no size" });
				setDrawing({ state: "drawing" });
				renderer.draw(figure.page, crop).then(
					(png) => setDrawing({ state: "drawn", url: URL.createObjectURL(png) }),
					(error: unknown) => setDrawing({ state: "failed", why: (error as Error).message }),
				);
			},
			{ rootMargin: "200px" },
		);
		seen.observe(element);
		return () => seen.disconnect();
	}, [renderer, figure, drawing.state]);

	// the image is freed with the figure: when the paper is closed
	const url = drawing.state === "drawn" ? drawing.url : null;
	useEffect(() => () => void (url && URL.revokeObjectURL(url)), [url]);

	useEffect(() => {
		if (!large) return;
		const close = (event: KeyboardEvent) => event.key === "Escape" && setLarge(false);
		window.addEventListener("keydown", close);
		return () => window.removeEventListener("keydown", close);
	}, [large]);

	const words = inside.join(" ").split(/\s+/).filter(Boolean).length;
	const name = `${figure.label} of the paper, as its PDF draws it`;
	return (
		<figure className="my-2 flex flex-col gap-1.5" data-figure={figure.label}>
			{drawing.state === "failed" ? (
				<p className="font-ui text-xs text-muted-foreground">
					{figure.label} could not be shown here: {drawing.why}.
				</p>
			) : (
				<div
					ref={box}
					className={cn("w-full overflow-hidden rounded-sm border bg-white", drawing.state !== "drawn" && "animate-pulse bg-muted")}
					style={{ aspectRatio: `${size.width} / ${size.height}` }}
				>
					{url && (
						<button type="button" className="block h-full w-full cursor-zoom-in" title="Show it larger" onClick={() => setLarge(true)}>
							<img src={url} alt={name} className="h-full w-full object-contain" draggable={false} />
						</button>
					)}
				</div>
			)}
			<figcaption>{caption}</figcaption>
			<div className="flex flex-wrap items-center gap-2 font-ui text-xs text-muted-foreground select-none">
				{onOpenPdf && (
					<Button
						variant="ghost"
						className="h-5 px-1.5 text-xs"
						title={`Open the PDF at page ${figure.page}`}
						onClick={() => onOpenPdf(figure.page)}
					>
						<FileText />
						see it in the paper, page {figure.page}
					</Button>
				)}
				{words > 0 && (
					<details className="group">
						<summary className="cursor-pointer">
							{words} {words === 1 ? "word" : "words"} inside the {figure.kind}
						</summary>
						<p className="mt-1 font-sans text-sm text-muted-foreground select-text">{inside.join(" · ")}</p>
					</details>
				)}
			</div>
			{large && url && (
				<div
					role="dialog"
					aria-label={name}
					className="fixed inset-0 z-50 flex cursor-zoom-out items-center justify-center bg-background/90 p-4"
					onClick={() => setLarge(false)}
				>
					<img src={url} alt={name} className="max-h-full max-w-full rounded-sm border bg-white object-contain" />
				</div>
			)}
		</figure>
	);
}

/** One renderer for the paper `pdfUrl` is the PDF of, made on first use and dropped with the paper. */
export function useFigureRenderer(pdfUrl: string, wanted: boolean): FigureRenderer | null {
	const [renderer, setRenderer] = useState<FigureRenderer | null>(null);
	useEffect(() => {
		if (!wanted) return;
		const made = new FigureRenderer(pdfUrl);
		setRenderer(made);
		return () => {
			made.dispose();
			setRenderer(null);
		};
	}, [pdfUrl, wanted]);
	return renderer;
}
