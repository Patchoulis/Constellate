"use client";

import { createElement, useCallback, useEffect, useRef, useState } from "react";
import { AlignLeft, ExternalLink, FileSearch, FileText, Loader2, MessagesSquare, Network, X } from "lucide-react";
import type { Catalog, CatalogItem, Declarations, Paper, Ratings, ReaderTarget } from "@rp/contracts";
import { AcceptanceTag, AiTag, FlagTag, RatingTags } from "@/components/brand";
import { Citations } from "@/components/citation-network";
import { CommunityDot, MarkButton } from "@/components/community-panel";
import { Weaknesses } from "@/components/critique";
import { MathText } from "@/components/math-text";
import { PaperText } from "@/components/paper-text";
import { ExplainFromText } from "@/components/passage-explainer";
import { PaperReviewFigures } from "@/components/review-summary";
import { Button, Heading, RowButton, Segmented } from "@/components/ui/button";
import { ResizeHandle } from "@/components/ui/resize-handle";
import { api } from "@/lib/api";
import { communityLabel, type CommunityNode } from "@/lib/community";
import { pdfAt } from "@/lib/evidence";
import { ready, useAsync, type Async } from "@/lib/use-async";
import { useResizable } from "@/lib/use-resizable";
import { count, similarity, SIMILARITY_NOTE } from "@/lib/units";
import { cn } from "@/lib/utils";

/** A paper the open one is linked to in the graph. */
export interface Related {
	id: string;
	title: string;
	similarity: number;
	community: CommunityNode;
}

type View = "abstract" | "paper" | "text" | "reviews" | "citations";

/**
 * Which tab a citation opens on. A place in the text lands on the Text tab, where the words are highlighted
 * (the browser's PDF viewer can only be opened at a page); a paper whose PDF the engine cannot read opens its PDF
 * at the page; evidence from the abstract opens the abstract.
 */
function viewFor(at: ReaderTarget | null | undefined, readable: boolean, hasPdf: boolean): View {
	if (!at || (!at.address && !at.quote)) return "abstract";
	if (readable) return "text";
	return at.address && hasPdf ? "paper" : "abstract";
}

/** The desktop app can show another site's page inside the window; a browser tab cannot (sites refuse frames). */
const EMBEDS = typeof navigator !== "undefined" && /Electron/.test(navigator.userAgent);

/**
 * Where each tab of the reader gets its content.
 *   paper    the PDF: from the backend when it can fetch it (arXiv, NeurIPS, ICML, CVPR...: kept locally),
 *            otherwise the paper's own PDF address, which only the desktop app's embedded view can show
 *   reviews  the paper's OpenReview page, where its reviews and discussion are
 * A tab with no source is not offered. The Citations tab is always offered: it is built from the reference lists
 * Constellate has read (components/citation-network.tsx).
 */
function sources(paper: Paper): { paper?: { url: string; embedded: boolean }; reviews?: { url: string; embedded: boolean } } {
	const remote = paper.pdf_url ?? undefined;
	return {
		paper: paper.readable ? { url: api.pdfUrl(paper.id), embedded: false } : remote ? { url: remote, embedded: true } : undefined,
		reviews: paper.url?.includes("openreview.net") ? { url: paper.url, embedded: true } : undefined,
	};
}

/** What the desktop app's embedded view offers beyond an ordinary element. */
interface EmbeddedView extends HTMLElement {
	executeJavaScript(code: string): Promise<unknown>;
	getURL(): string;
}

const LOOKS = 12; // times the shown page's text is read, a few seconds apart: reviews arrive after the page does

/**
 * Another site's page, shown by the desktop app's embedded browser view. With `onText`, the text of the page as
 * shown is handed over once it has loaded, and again until `onText` says it has what it wanted: this is how
 * a paper's ratings are read from its reviews page, which the user opened and a program may not fetch.
 */
function Embedded({ url, onText }: { url: string; onText?: (text: string, shown: string) => Promise<boolean> }) {
	const ref = useRef<EmbeddedView | null>(null);
	useEffect(() => {
		const view = ref.current;
		if (!view || !onText) return;
		let looks = 0;
		let over = false;
		let timer: ReturnType<typeof setTimeout> | undefined;
		const look = async () => {
			if (over) return;
			try {
				// the page says what its text is, so it is treated as untrusted: capped here, parsed and checked by the backend
				const text = await view.executeJavaScript("String(document.body.innerText).slice(0, 1500000)");
				if (typeof text === "string" && (await onText(text.slice(0, 1_500_000), view.getURL()))) return;
			} catch {
				// the view is navigating: look again
			}
			if (!over && ++looks < LOOKS) timer = setTimeout(look, 2500);
		};
		const loaded = () => {
			looks = 0;
			clearTimeout(timer);
			timer = setTimeout(look, 1200);
		};
		view.addEventListener("did-stop-loading", loaded);
		return () => {
			over = true;
			clearTimeout(timer);
			view.removeEventListener("did-stop-loading", loaded);
		};
	}, [onText]);
	// its own storage partition: the shell refuses an embedded view without it (apps/desktop/main.cjs)
	return createElement("webview", { ref, src: url, partition: "reader", plugins: "true", className: "min-h-0 flex-1 bg-background" });
}

/** A page shown inside the reader: a frame for our own backend, an embedded browser view for another site. */
function Page({
	source,
	title,
	onText,
}: {
	source: { url: string; embedded: boolean };
	title: string;
	onText?: (text: string, shown: string) => Promise<boolean>;
}) {
	if (!source.embedded) return <iframe src={source.url} title={title} className="min-h-0 flex-1 bg-background" />;
	if (EMBEDS) return <Embedded url={source.url} onText={onText} />;
	return (
		<div className="flex flex-1 flex-col items-center justify-center gap-3 p-8 text-center text-sm text-muted-foreground">
			This page can only be shown inside the desktop app.
			<Button variant="outline" onClick={() => window.open(source.url, "_blank", "noopener")}>
				<ExternalLink />
				Open it in the browser
			</Button>
		</div>
	);
}

const ANSWER = { Yes: "text-success", No: "text-warning", NA: "text-muted-foreground" } as const;

/** What the paper's authors declare in it, read from its PDF on request. */
function Declared({
	paper,
	flagNames,
}: {
	paper: Paper;
	/** flag id -> its entry in the catalog: what it means (label) and what raises it (note) */
	flagNames: Record<string, CatalogItem>;
}) {
	const [asked, setAsked] = useState(false);
	const read: Async<Declarations> = useAsync(asked ? () => api.declarations(paper.id) : null, [asked, paper.id]);
	const declared = paper.declarations ?? ready(read);
	if (!declared) {
		if (!paper.readable)
			return (
				<section className="flex flex-col gap-1.5">
					<Heading className="font-ui">What the authors declare</Heading>
					<p className="text-base text-muted-foreground">
						Not read: this paper&apos;s PDF is not published anywhere a program may fetch it from, so its checklist and statements are
						unknown here.
					</p>
				</section>
			);
		return (
			<section className="flex flex-col items-start gap-2 font-ui">
				<Heading>What the authors declare</Heading>
				<Button variant="outline" disabled={read.status === "loading"} onClick={() => setAsked(true)}>
					{read.status === "loading" ? <Loader2 className="animate-spin" /> : <FileSearch />}
					{read.status === "loading" ? "Reading the paper" : "Read the checklist and statements"}
				</Button>
				{read.status === "failed" && <p className="text-xs text-destructive">{read.error}</p>}
			</section>
		);
	}
	const nothing = declared.checklist.length === 0 && declared.statements.length === 0;
	return (
		<section className="flex flex-col gap-3">
			<Heading className="font-ui">What the authors declare</Heading>
			{declared.flags.length > 0 && (
				<div className="flex flex-wrap gap-1.5">
					{declared.flags.map((flag) => (
						<FlagTag key={flag} note={flagNames[flag]?.note}>
							{flagNames[flag]?.label ?? flag}
						</FlagTag>
					))}
				</div>
			)}
			{nothing && <p className="text-base text-muted-foreground">No checklist and no statement sections were found in this paper.</p>}
			{declared.statements.map((statement) => (
				<div key={statement.kind}>
					<div className="font-ui text-xs text-muted-foreground">{statement.title}</div>
					<p className="text-base text-foreground/90">
						<MathText>{statement.text}</MathText>
					</p>
				</div>
			))}
			{declared.checklist.length > 0 && (
				<ol className="flex flex-col gap-2">
					{declared.checklist.map((item) => (
						<li key={item.number} className="grid grid-cols-[2.5rem_1fr] gap-x-2">
							<span className={cn("font-ui text-xs font-semibold", ANSWER[item.answer])}>{item.answer}</span>
							<span className="text-base leading-snug">
								{item.title}
								{item.justification && <span className="block text-sm text-muted-foreground">{item.justification}</span>}
							</span>
						</li>
					))}
				</ol>
			)}
		</section>
	);
}

function Abstract({
	paper,
	community,
	related,
	flagNames,
	ratings,
	onOpen,
	onRead,
	onReviews,
}: {
	paper: Paper;
	community?: CommunityNode;
	related: Related[];
	flagNames: Record<string, CatalogItem>;
	ratings: Ratings | null;
	/** open the reviews page (which is also what reads the ratings); absent when there is none to open */
	onReviews?: () => void;
	onOpen: (id: string) => void;
	onRead?: () => void;
}) {
	return (
		<article className="mx-auto flex max-w-[68ch] flex-col gap-4 font-sans">
			{community && (
				<div className="flex items-center gap-2 font-ui text-xs text-muted-foreground">
					<CommunityDot hue={community.hue} />
					{communityLabel(community.community)}
				</div>
			)}
			<h1 className="text-xl font-semibold text-balance">
				<MathText>{paper.title}</MathText>
			</h1>
			<p className="text-base text-muted-foreground">{paper.authors.join(", ")}</p>
			<div className="flex flex-wrap items-center gap-x-4 gap-y-1.5 font-ui text-xs text-muted-foreground">
				<span>{paper.published.slice(0, 10)}</span>
				<span>{paper.categories.join(" · ")}</span>
				{paper.venues?.map((venue) => (
					<span key={venue}>{venue}</span>
				))}
				<AcceptanceTag state={paper.acceptance} term={paper.acceptance_term} />
				{paper.ai?.flagged && <AiTag verdict={paper.ai} />}
			</div>
			{ratings ? (
				<div className="flex flex-wrap items-center gap-1.5 font-ui text-xs text-muted-foreground">
					<span
						title={
							"Mean score per field; hover a score for each reviewer's." +
							(ratings.exact
								? ""
								: " The reviews could not be told apart on this page, so the count is a guess and quoted scores may be included.")
						}
					>
						{ratings.exact ? "" : "about "}
						{count(ratings.reviews, "review")}
					</span>
					<RatingTags ratings={ratings} />
				</div>
			) : (
				onReviews && (
					<button
						type="button"
						className="self-start font-ui text-xs text-muted-foreground underline-offset-2 hover:underline"
						onClick={onReviews}
					>
						Reviewers&apos; ratings: open the Reviews tab once to read them
					</button>
				)
			)}
			<p className="text-lg leading-relaxed text-foreground">
				<MathText>{paper.abstract || "This paper's abstract is not in the list it came from."}</MathText>
			</p>
			{onRead && (
				<div>
					<Button variant="outline" className="font-ui" onClick={onRead}>
						<FileText />
						Read the paper
					</Button>
				</div>
			)}
			<Declared paper={paper} flagNames={flagNames} />
			<Weaknesses id={paper.id} onReviews={onReviews} />
			{related.length > 0 && (
				<section className="mt-2">
					<Heading className="mb-1.5 font-ui">Closest papers</Heading>
					{related.map((other) => (
						<RowButton key={other.id} className="items-baseline gap-2" onClick={() => onOpen(other.id)}>
							<span className="translate-y-px">
								<CommunityDot hue={other.community.hue} />
							</span>
							<span className="flex-1 text-base leading-snug">
								<MathText>{other.title}</MathText>
							</span>
							<span className="shrink-0 font-ui text-xs text-muted-foreground tabular-nums" title={SIMILARITY_NOTE}>
								{similarity(other.similarity)}
							</span>
						</RowButton>
					))}
				</section>
			)}
		</article>
	);
}

/** The paper pulled out of the graph: its record, the paper itself, and its reviews, each a tab of this panel.
 *  Mount it with key={id}, so a different paper starts on its abstract again. */
export function PaperReader({
	id,
	community,
	related,
	catalog,
	marked,
	onToggleMark,
	run,
	at,
	onRatings,
	onOpen,
	onClose,
}: {
	id: string;
	/** a place in the paper to open at and the words to highlight there: where a citation (EvidenceLink) leads */
	at?: ReaderTarget | null;
	community?: CommunityNode;
	related: Related[];
	catalog: Catalog | undefined;
	/** has the user marked this paper in this run? */
	marked: boolean;
	onToggleMark: () => void;
	/** the run the paper is opened from: its Citations tab counts the citing papers of that run */
	run?: string;
	/** this paper's ratings were just read from its reviews page */
	onRatings?: (id: string, ratings: Ratings) => void;
	onOpen: (id: string) => void;
	onClose: () => void;
}) {
	const paper = useAsync(() => api.paper(id), [id]);
	const [read, setRead] = useState<Ratings | null>(null);
	const seen = useRef(-1); // reviews found at the last look: reading stops once two looks agree
	const readRatings = useCallback(
		async (text: string, shown: string) => {
			const found = await api.readRatings(id, text, shown).catch(() => null);
			if (!found) return false;
			setRead(found);
			onRatings?.(id, found);
			const settled = found.reviews === seen.current;
			seen.current = found.reviews;
			return settled;
		},
		[id, onRatings],
	);
	const [view, setView] = useState<View>("abstract");
	const found = ready(paper);
	const from = found ? sources(found) : {};
	// the page the PDF is opened at: the cited place's, or one the Text tab asked for
	const [pdfPage, setPdfPage] = useState<number | undefined>(at?.address?.page);
	// a citation opens its tab once the paper is known, and again whenever it leads elsewhere in the same paper
	const readable = found?.readable ?? false;
	const hasPdf = Boolean(from.paper);
	useEffect(() => {
		if (!found) return;
		setView(viewFor(at, readable, hasPdf));
		setPdfPage(at?.address?.page);
	}, [at, found, readable, hasPdf]);
	const tabs = [
		{ value: "abstract" as const, label: "Abstract" },
		...(from.paper ? [{ value: "paper" as const, label: "Paper", icon: <FileText /> }] : []),
		...(found?.readable
			? [{ value: "text" as const, label: "Text", icon: <AlignLeft />, title: "The paper's text as read from its PDF, by section" }]
			: []),
		...(from.reviews ? [{ value: "reviews" as const, label: "Reviews", icon: <MessagesSquare /> }] : []),
		...(found ? [{ value: "citations" as const, label: "Citations", icon: <Network /> }] : []),
	];
	const flagNames = Object.fromEntries((catalog?.flags ?? []).flatMap((group) => group.items).map((item) => [item.name, item]));
	const pdf = from.paper && { ...from.paper, url: pdfAt(from.paper.url, pdfPage) };
	const shown = view === "paper" ? pdf : view === "reviews" ? from.reviews : undefined;
	const openPdf = (page: number) => {
		setPdfPage(page);
		setView("paper");
	};

	// it lies over the right of the graph, which moves a paper into the rest (graph-view reads its width)
	const resizable = useResizable<HTMLDivElement>("reader");

	useEffect(() => {
		const onKey = (event: KeyboardEvent) => event.key === "Escape" && onClose();
		window.addEventListener("keydown", onKey);
		return () => window.removeEventListener("keydown", onKey);
	}, [onClose]);

	return (
		<div
			ref={resizable.ref}
			style={resizable.style}
			className="enter-from-right absolute inset-y-0 right-0 z-20 flex w-[min(var(--panel-size),100%)] flex-col border-l bg-card shadow-2xl"
		>
			<ResizeHandle {...resizable.handle} />
			<div className="flex h-11 shrink-0 items-center gap-2 border-b px-3">
				{tabs.length > 1 && <Segmented options={tabs} value={view} onChange={setView} />}
				<span className="min-w-0 flex-1 truncate text-xs text-muted-foreground">{found ? found.title : id}</span>
				<MarkButton marked={marked} onToggle={onToggleMark} />
				{found?.url && (
					<Button
						variant="ghost"
						size="icon"
						title="Open the paper's page in the browser"
						onClick={() => window.open(found.url, "_blank", "noopener")}
					>
						<ExternalLink />
					</Button>
				)}
				<Button variant="ghost" size="icon" title="Close (Esc)" onClick={onClose}>
					<X />
				</Button>
			</div>
			{view === "reviews" && <PaperReviewFigures id={id} version={read} />}
			{view === "paper" && found?.readable && <ExplainFromText onText={() => setView("text")} />}
			{shown && found ? (
				<Page
					key={`${view}:${shown.url}`}
					source={shown}
					title={`${found.title}: ${view}`}
					onText={view === "reviews" ? readRatings : undefined}
				/>
			) : (
				<div className="min-h-0 flex-1 overflow-y-auto px-8 py-6">
					{paper.status === "failed" && <p className="text-sm text-destructive">{paper.error}</p>}
					{paper.status === "loading" && <p className="text-sm text-muted-foreground">Loading…</p>}
					{found && view === "citations" && <Citations paper={found} run={run} onOpen={onOpen} />}
					{found && view === "text" && (
						// a new citation starts afresh: nothing of the last one may stay highlighted while this one is found
						<PaperText key={JSON.stringify(at ?? null)} id={found.id} at={at} onOpenPdf={from.paper ? openPdf : undefined} run={run} />
					)}
					{found && view !== "citations" && view !== "text" && (
						<Abstract
							paper={found}
							community={community}
							related={related}
							flagNames={flagNames}
							ratings={read ?? found.reviews}
							onReviews={from.reviews && EMBEDS ? () => setView("reviews") : undefined}
							onOpen={onOpen}
							onRead={from.paper ? () => openPdf(1) : undefined}
						/>
					)}
				</div>
			)}
		</div>
	);
}
