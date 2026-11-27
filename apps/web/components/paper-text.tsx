"use client";

import { Fragment, useEffect, useRef, useState } from "react";
import { AlignLeft, FileText, MessageSquareText, Sparkles } from "lucide-react";
import type { Passage, PassageAddress, PassageKind } from "@rp/contracts";
import { PassageExplainer, type AskedOnce, type ExplainMode } from "@/components/passage-explainer";
import { Button } from "@/components/ui/button";
import { api } from "@/lib/api";
import { highlighted, pages, passageAt } from "@/lib/evidence";
import { explainable, placeKey, resolveSelection, selectionQuote } from "@/lib/explanation";
import { noReadablePdf } from "@/lib/no-pdf";
import { ready, useAsync } from "@/lib/use-async";
import { cn } from "@/lib/utils";

/** What a passage that is not running prose is, said beside it: the PDF's text for these is a reading of it. */
const KIND_NOTE: Partial<Record<PassageKind, { label: string; note: string }>> = {
	figure_caption: { label: "Figure caption", note: "A figure's caption, as read from the PDF." },
	table_caption: { label: "Table caption", note: "A table's caption, as read from the PDF." },
	equation: { label: "Equation", note: "An equation as the PDF's text has it: symbols and layout are often lost." },
	footnote: { label: "Footnote", note: "A footnote, affiliation or the venue's notice, as read from the PDF." },
	fragment: { label: "Figure or table text", note: "Text drawn inside a figure or a table, or a list of names, as read from the PDF." },
};

const PART_NAME = { appendix: "Appendix", checklist: "Checklist" } as const;

function Shown({ passage, mark, markRef }: { passage: Passage; mark: PassageAddress | null; markRef?: React.Ref<HTMLElement> }) {
	const parts = highlighted(passage, mark);
	const text = parts ? (
		<>
			{parts[0]}
			<mark ref={markRef} className="rounded-sm bg-primary/20 px-0.5 text-foreground select-text">
				{parts[1]}
			</mark>
			{parts[2]}
		</>
	) : (
		passage.text
	);
	if (passage.kind === "heading" || passage.kind === "title") {
		return <h3 className={cn("mt-4 font-semibold text-balance", passage.kind === "title" ? "text-xl" : "text-base")}>{text}</h3>;
	}
	const kind = KIND_NOTE[passage.kind];
	return (
		<p
			className={cn(
				"text-base leading-relaxed",
				kind && "text-sm text-muted-foreground",
				passage.kind === "equation" && "font-mono text-xs",
			)}
		>
			{kind && (
				// drawn by CSS, not text of the page: a selection of the whole passage is the paper's words only
				<span
					title={kind.note}
					data-label={kind.label}
					className="mr-1.5 font-ui text-[0.65rem] tracking-wide uppercase select-none before:content-[attr(data-label)]"
				/>
			)}
			{text}
		</p>
	);
}

/** The passage explained or searched from, how it was chosen, and which of the two. */
interface Chosen {
	index: number;
	mode: ExplainMode;
	note: string | null;
}

/** Words selected in the text, offered to be explained: where to show the offer, and in which passage they start. */
interface Offer {
	quote: string;
	within: number | null;
	spans: boolean;
	top: number;
	left: number;
}

/** The passage element a node of the page is in, if any. */
function passageOf(node: Node | null): Element | null {
	const element = node instanceof Element ? node : (node?.parentElement ?? null);
	return element?.closest("[data-passage]") ?? null;
}

/**
 * The paper's text as read from its PDF, section by section, with page boundaries marked: what a citation lands
 * on when the PDF viewer cannot be asked to highlight a passage. The cited words are marked and scrolled into
 * view; `at.quote` without an address is looked for in the paper first.
 *
 * Words selected here (with the mouse or the keyboard), or a passage's own button, can be explained by the model
 * or matched with passages of nearby papers (components/passage-explainer.tsx). Both are offered once the
 * explanations kept for the paper have been read.
 */
export function PaperText({
	id,
	at,
	onOpenPdf,
	run,
}: {
	id: string;
	at?: { address?: PassageAddress | null; quote?: string | null } | null;
	/** open the PDF at a page; absent when the PDF cannot be shown */
	onOpenPdf?: (page: number) => void;
	/** the run the paper is read from: where related passages are looked for */
	run?: string;
}) {
	const read = useAsync(() => api.passages(id), [id], { keep: false });
	const quote = at?.address ? null : (at?.quote ?? null);
	// where another quote was found is never this one's place
	const located = useAsync(quote ? () => api.locate(id, quote) : null, [id, quote], { keep: false });
	const found = ready(located);
	const mark = at?.address ?? (found?.found ? found.address : null);
	const markRef = useRef<HTMLElement | null>(null);
	const passages = ready(read);
	const target = passages && mark ? passageAt(passages, mark) : -1;

	// the passages explained before; a backend without the route has no explaining at all
	const marks = useAsync(() => api.explainedPassages(id), [id], { keep: false });
	const explaining = ready(marks);
	const explained = new Set(explaining?.explained.map(placeKey) ?? []);
	const [chosen, setChosen] = useState<Chosen | null>(null);
	const [offer, setOffer] = useState<Offer | null>(null);
	const [refused, setRefused] = useState<{ why: string; top: number } | null>(null);
	const articleRef = useRef<HTMLElement | null>(null);
	// passages asked about without a confirmation while this paper is open: never twice (passage-explainer.tsx)
	const askedOnce = useRef<AskedOnce>(new Set());

	useEffect(() => {
		markRef.current?.scrollIntoView({ block: "center" });
	}, [target, mark]);

	// a selection in the text (by mouse or keyboard) is offered to be explained, once it settles
	const enabled = explaining !== undefined;
	useEffect(() => {
		if (!enabled) return;
		let timer: ReturnType<typeof setTimeout> | undefined;
		const look = () => {
			const article = articleRef.current;
			const selection = window.getSelection();
			if (!article || !selection || selection.isCollapsed || selection.rangeCount === 0) return setOffer(null);
			const range = selection.getRangeAt(0);
			const start = passageOf(range.startContainer);
			const end = passageOf(range.endContainer);
			// only words of the paper's text, not outside the article
			if (!start || !article.contains(start)) return setOffer(null);
			// what is looked for is cut at the end of the passage it starts in: page dividers and the next passage
			// are not words of it. A triple-click ends at the very start of the next passage, which is no spanning.
			const words = range.cloneRange();
			let spans = false;
			if (end !== start) {
				words.setEndAfter(start);
				if (end) {
					const tail = document.createRange();
					tail.setStart(end, 0);
					tail.setEnd(range.endContainer, range.endOffset);
					spans = tail.toString().trim() !== "";
				} else spans = true;
			}
			const box = range.getBoundingClientRect();
			const frame = article.getBoundingClientRect();
			setRefused(null);
			setOffer({
				quote: words.toString(),
				within: Number(start.getAttribute("data-passage")),
				spans,
				top: box.bottom - frame.top + 4,
				left: Math.max(0, box.left - frame.left),
			});
		};
		const settle = () => {
			clearTimeout(timer);
			timer = setTimeout(look, 200);
		};
		document.addEventListener("selectionchange", settle);
		return () => {
			clearTimeout(timer);
			document.removeEventListener("selectionchange", settle);
		};
	}, [enabled]);

	const choose = async (mode: ExplainMode) => {
		if (!offer || !passages) return;
		const place = { top: offer.top };
		setOffer(null);
		const taken = selectionQuote(offer.quote);
		if (!taken) return;
		if ("refused" in taken) return setRefused({ why: taken.refused, ...place });
		try {
			const resolution = resolveSelection(passages, await api.locate(id, taken.quote), offer.within);
			if (resolution.kind === "refused") return setRefused({ why: resolution.why, ...place });
			setRefused(null);
			const note = offer.spans ? "The selection runs past this paragraph: the paragraph it starts in is explained." : resolution.note;
			setChosen({ index: resolution.index, mode, note });
		} catch (error) {
			setRefused({ why: (error as Error).message, ...place });
		}
	};

	if (read.status === "failed")
		// no PDF the engine can have or read (it answers 404): nothing went wrong
		return noReadablePdf(read.error) ? (
			<p className="text-sm text-muted-foreground">
				The paper&apos;s text is not available here{onOpenPdf ? ": its PDF is in the Paper tab." : "."}
			</p>
		) : (
			<p className="text-sm text-destructive">{read.error}</p>
		);
	if (!passages) return <p className="text-sm text-muted-foreground">Reading the paper…</p>;
	return (
		<article ref={articleRef} className="relative mx-auto flex max-w-[68ch] flex-col gap-3 font-sans select-text">
			<p className="font-ui text-xs text-muted-foreground" title="Read by a program from the PDF: check the PDF itself where it matters.">
				The paper&apos;s text as read from its PDF. Running headers, page numbers and the reference list are left out.
				{enabled && " Select words, or use a passage's button, to have it explained."}
			</p>
			{quote && found && !found.found && (
				<p className="font-ui text-xs text-warning">These words were not found in the paper&apos;s text: nothing is highlighted.</p>
			)}
			{offer && (
				<div
					className="absolute z-10 flex gap-1 rounded-md border bg-card p-0.5 shadow-md"
					style={{ top: offer.top, left: offer.left }}
					// keep the selection while a button is pressed
					onMouseDown={(event) => event.preventDefault()}
				>
					<Button
						variant="ghost"
						className="h-6 px-2 text-xs"
						title="The model explains the passage these words are in (its cost is shown first)"
						onClick={() => void choose("explain")}
					>
						<Sparkles />
						Explain
					</Button>
					{run && (
						<Button
							variant="ghost"
							className="h-6 px-2 text-xs"
							title="Passages of nearby papers sharing its words: free"
							onClick={() => void choose("related")}
						>
							<AlignLeft />
							Find related
						</Button>
					)}
				</div>
			)}
			{refused && (
				<button
					type="button"
					title="Close"
					className="absolute z-10 rounded-md border bg-card px-2 py-1 text-left font-ui text-xs text-warning shadow-md"
					style={{ top: refused.top, left: 0 }}
					onClick={() => setRefused(null)}
				>
					{refused.why}
				</button>
			)}
			{passages.passages.map((passage, index) => {
				const previous = passages.passages[index - 1];
				const newPage = !previous || previous.address.page !== passage.address.page;
				const newPart = passage.part in PART_NAME && previous?.part !== passage.part;
				const canExplain = enabled && explainable(passage.kind);
				const kept = explained.has(placeKey(passage.address));
				return (
					<Fragment key={`${passage.address.page}-${passage.address.start}`}>
						{newPart && (
							<div className="mt-6 border-t pt-2 font-ui text-xs tracking-wide text-muted-foreground uppercase select-none">
								{PART_NAME[passage.part as keyof typeof PART_NAME]}
							</div>
						)}
						{newPage && (
							<div className="flex items-center gap-2 font-ui text-xs text-muted-foreground select-none">
								<span className="h-px flex-1 bg-border" />
								<span>{pages(passage.address)}</span>
								{onOpenPdf && (
									<Button
										variant="ghost"
										className="h-5 px-1.5 text-xs"
										title={`Open the PDF at page ${passage.address.page}`}
										onClick={() => onOpenPdf(passage.address.page)}
									>
										<FileText />
										PDF
									</Button>
								)}
								<span className="h-px flex-1 bg-border" />
							</div>
						)}
						<div data-passage={index} className="group relative">
							{canExplain && (
								<button
									type="button"
									title={kept ? "An explanation of this passage is kept: open it (free)" : "Explain this passage, or find related passages"}
									onClick={() => setChosen({ index, mode: "explain", note: null })}
									className={cn(
										"absolute top-1 -left-6 text-muted-foreground transition-opacity select-none hover:text-foreground focus-visible:opacity-100 [&_svg]:size-3.5",
										kept ? "text-primary opacity-80" : "opacity-0 group-hover:opacity-100",
									)}
								>
									{kept ? <MessageSquareText /> : <Sparkles />}
								</button>
							)}
							<Shown passage={passage} mark={index === target ? mark : null} markRef={index === target ? markRef : undefined} />
						</div>
						{chosen?.index === index && (
							<div data-explainer>
								<PassageExplainer
									paper={id}
									run={run}
									passage={passage}
									mode={chosen.mode}
									note={chosen.note}
									onMode={(mode) => setChosen({ ...chosen, mode })}
									onClose={() => setChosen(null)}
									onExplained={marks.reload}
									askedOnce={askedOnce.current}
								/>
							</div>
						)}
					</Fragment>
				);
			})}
		</article>
	);
}
