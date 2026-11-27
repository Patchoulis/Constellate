"use client";

import { Fragment, useEffect, useRef } from "react";
import { FileText } from "lucide-react";
import type { Passage, PassageAddress, PassageKind } from "@rp/contracts";
import { Button } from "@/components/ui/button";
import { api } from "@/lib/api";
import { highlighted, pages, passageAt } from "@/lib/evidence";
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
				<span title={kind.note} className="mr-1.5 font-ui text-[0.65rem] tracking-wide uppercase">
					{kind.label}
				</span>
			)}
			{text}
		</p>
	);
}

/**
 * The paper's text as read from its PDF, section by section, with page boundaries marked: what a citation lands
 * on when the PDF viewer cannot be asked to highlight a passage. The cited words are marked and scrolled into
 * view; `at.quote` without an address is looked for in the paper first.
 */
export function PaperText({
	id,
	at,
	onOpenPdf,
}: {
	id: string;
	at?: { address?: PassageAddress | null; quote?: string | null } | null;
	/** open the PDF at a page; absent when the PDF cannot be shown */
	onOpenPdf?: (page: number) => void;
}) {
	const read = useAsync(() => api.passages(id), [id]);
	const quote = at?.address ? null : (at?.quote ?? null);
	const located = useAsync(quote ? () => api.locate(id, quote) : null, [id, quote]);
	const found = ready(located);
	const mark = at?.address ?? (found?.found ? found.address : null);
	const markRef = useRef<HTMLElement | null>(null);
	const passages = ready(read);
	const target = passages && mark ? passageAt(passages, mark) : -1;

	useEffect(() => {
		markRef.current?.scrollIntoView({ block: "center" });
	}, [target, mark]);

	if (read.status === "failed") return <p className="text-sm text-destructive">{read.error}</p>;
	if (!passages) return <p className="text-sm text-muted-foreground">Reading the paper…</p>;
	return (
		<article className="mx-auto flex max-w-[68ch] flex-col gap-3 font-sans select-text">
			<p className="font-ui text-xs text-muted-foreground" title="Read by a program from the PDF: check the PDF itself where it matters.">
				The paper&apos;s text as read from its PDF. Running headers, page numbers and the reference list are left out.
			</p>
			{quote && found && !found.found && (
				<p className="font-ui text-xs text-warning">These words were not found in the paper&apos;s text: nothing is highlighted.</p>
			)}
			{passages.passages.map((passage, index) => {
				const previous = passages.passages[index - 1];
				const newPage = !previous || previous.address.page !== passage.address.page;
				const newPart = passage.part in PART_NAME && previous?.part !== passage.part;
				return (
					<Fragment key={`${passage.address.page}-${passage.address.start}`}>
						{newPart && (
							<div className="mt-6 border-t pt-2 font-ui text-xs tracking-wide text-muted-foreground uppercase">
								{PART_NAME[passage.part as keyof typeof PART_NAME]}
							</div>
						)}
						{newPage && (
							<div className="flex items-center gap-2 font-ui text-xs text-muted-foreground">
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
						<Shown passage={passage} mark={index === target ? mark : null} markRef={index === target ? markRef : undefined} />
					</Fragment>
				);
			})}
		</article>
	);
}
