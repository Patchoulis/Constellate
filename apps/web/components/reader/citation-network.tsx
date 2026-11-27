"use client";

import { useEffect, useMemo, useRef, useState } from "react";
import { BookOpenText, Loader2, RotateCw } from "lucide-react";
import type { CitationKind, CitationNeighbour, CitationNetwork, Paper } from "@rp/contracts";
import { contextsOf } from "@/components/features/citation_contexts/citation-contexts";
import { KIND_HUE, edgeKinds, lineKind } from "@/lib/features/citation_contexts/citation-kinds";
import { MathText } from "@/components/math-text";
import { Button, Heading, RowButton, Segmented } from "@/components/ui/button";
import { Input } from "@/components/ui/field";
import { api } from "@/lib/net/api";
import { openOutside } from "@/lib/net/links";
import {
	authorLine,
	citedByLine,
	confidenceNote,
	layoutNetwork,
	linkTag,
	lookupsStoppedLine,
	outsideLink,
	showing,
	shownWorks,
	type PlacedWork,
	type WorkOrder,
} from "@/lib/reader/citations";
import { placeLabels } from "@/lib/graph/labels";
import { count } from "@/lib/units";
import { ready, useAsync } from "@/lib/use-async";
import { cn, hsl } from "@/lib/utils";

const HEIGHT = 340; // px of the picture
const AXIS = 18; // px under the works for the years
const HUE = { seed: 42, prior: 205, derivative: 150 } as const;
const LABELLED = 6; // works named on the picture, the most cited first
const LABEL_CHARS = 30;
const CHAR_PX = 6.2; // width of one character of the 10px label font, near enough to keep labels apart
const TAG = "inline-flex items-center rounded-sm border px-1.5 py-px font-ui text-xs";

/** Open a work: in the reader when it is in the library, otherwise in the browser. */
function opener(onOpen: (id: string) => void) {
	return (work: CitationNeighbour) => (work.link.kind === "in_library" ? onOpen(work.link.paper) : openOutside(outsideLink(work)));
}

/** What a work was found to be, as a tag that explains itself on hover. */
function LinkTag({ work }: { work: CitationNeighbour }) {
	const tag = linkTag(work.link);
	const note = confidenceNote(work.confidence);
	return (
		<span
			title={note ? `${tag.note} ${note}` : tag.note}
			className={cn(
				TAG,
				work.link.kind === "in_library" ? "border-foreground/40 text-foreground" : "border-border text-muted-foreground",
				work.link.kind === "unresolved" && "border-dashed",
			)}
		>
			{tag.label}
		</span>
	);
}

/** The width of the element, followed as it changes. */
function useWidth() {
	const ref = useRef<HTMLDivElement>(null);
	const [width, setWidth] = useState(0);
	useEffect(() => {
		const element = ref.current;
		if (!element) return;
		const observer = new ResizeObserver(() => setWidth(element.clientWidth));
		observer.observe(element);
		setWidth(element.clientWidth);
		return () => observer.disconnect();
	}, []);
	return [ref, width] as const;
}

/** How a work is drawn: filled when it is in the library, a ring when it is on arXiv, a dashed ring when not found. */
function Work({ work, hovered }: { work: PlacedWork; hovered: boolean }) {
	const colour = hsl(HUE[work.side], hovered ? 78 : 66);
	if (!work.work)
		return <circle cx={work.x} cy={work.y} r={work.r} fill={colour} style={{ filter: `drop-shadow(0 0 6px ${hsl(HUE.seed, 60, 0.8)})` }} />;
	const kind = work.work.link.kind;
	return (
		<circle
			cx={work.x}
			cy={work.y}
			r={work.r}
			fill={kind === "in_library" ? colour : "transparent"}
			stroke={kind === "in_library" ? "none" : kind === "on_arxiv" ? colour : "#8d8d8d"}
			strokeWidth={1.4}
			strokeDasharray={kind === "unresolved" ? "2 2" : undefined}
			style={kind === "in_library" ? { filter: `drop-shadow(0 0 4px ${hsl(HUE[work.side], 60, 0.6)})` } : undefined}
		/>
	);
}

/** The network as a picture: time left to right, the seed in the middle. */
function Picture({
	network,
	seed,
	onOpen,
	kinds,
}: {
	network: CitationNetwork;
	seed: Paper;
	onOpen: (work: CitationNeighbour) => void;
	kinds?: Map<string, CitationKind>;
}) {
	const [box, width] = useWidth();
	const [hovered, setHovered] = useState<number | null>(null);
	const seedYear = Number(seed.published.slice(0, 4)) || null;
	const frameWidth = Math.max(width, 320);
	const frame = useMemo(() => ({ width: frameWidth, height: HEIGHT - AXIS }), [frameWidth]);
	const picture = useMemo(() => layoutNetwork(network, seedYear, frame), [network, seedYear, frame]);
	const labels = useMemo(() => {
		const named = picture.works
			.map((work, index) => ({ work, index }))
			.filter(({ work }) => work.work)
			.sort((a, b) => b.work.work!.cited_by - a.work.work!.cited_by)
			.slice(0, LABELLED);
		const texts = named.map(({ work }) => {
			const title = work.work!.title;
			return title.length > LABEL_CHARS ? `${title.slice(0, LABEL_CHARS - 1)}…` : title;
		});
		const places = placeLabels(
			frame,
			named.map(({ work }, at) => ({ x: work.x, y: work.y, width: texts[at].length * CHAR_PX, height: 12, gap: work.r + 3 })),
		);
		return named.map(({ index }, at) => ({ index, text: texts[at], ...places[at] }));
	}, [picture, frame]);
	const touching = (line: { from: number; to: number }) => hovered !== null && (line.from === hovered || line.to === hovered);
	const shown = hovered !== null ? picture.works[hovered] : null;

	return (
		<figure className="flex flex-col gap-2">
			<div ref={box} className="relative w-full overflow-hidden rounded-lg border bg-background" style={{ height: HEIGHT }}>
				{width > 0 && (
					<svg width={frame.width} height={HEIGHT} className="block" onMouseLeave={() => setHovered(null)}>
						{picture.lines.map((line, at) => {
							const from = picture.works[line.from];
							const to = picture.works[line.to];
							const lit = touching(line);
							const kind = lineKind(picture.works, line, kinds);
							const stroke = lit
								? "rgba(255,255,255,0.75)"
								: kind
									? hsl(KIND_HUE[kind] ?? 0, 62, 0.6)
									: line.seed
										? "rgba(255,255,255,0.05)"
										: "rgba(255,255,255,0.2)";
							return <line key={at} x1={from.x} y1={from.y} x2={to.x} y2={to.y} stroke={stroke} strokeWidth={lit ? 1.2 : 0.8} />;
						})}
						{picture.works.map((work, index) => (
							<g
								key={work.key}
								className={work.work ? "cursor-pointer" : undefined}
								onMouseEnter={() => setHovered(index)}
								onMouseLeave={() => setHovered(null)}
								onClick={() => work.work && onOpen(work.work)}
							>
								<circle cx={work.x} cy={work.y} r={work.r + 3} fill="transparent" />
								<Work work={work} hovered={hovered === index} />
							</g>
						))}
						{labels.map((label) => (
							<text
								key={label.index}
								x={label.left}
								y={label.top + 9}
								className="pointer-events-none fill-muted-foreground font-ui text-[10px]"
							>
								{label.text}
							</text>
						))}
						<line x1={0} x2={frame.width} y1={frame.height + 2} y2={frame.height + 2} stroke="rgba(255,255,255,0.12)" />
						{picture.ticks.map((tick) => (
							<text
								key={`${tick.year}-${tick.x}`}
								x={tick.x}
								y={HEIGHT - 4}
								textAnchor="middle"
								className={cn("font-ui text-[10px]", tick.year === seedYear ? "fill-foreground" : "fill-muted-foreground")}
							>
								{tick.year ?? "no year"}
							</text>
						))}
					</svg>
				)}
				{shown && (
					<div
						className="pointer-events-none absolute z-10 max-w-80 rounded-md border bg-popover px-3 py-2 text-xs shadow-xl"
						style={{
							left: Math.max(6, Math.min(shown.x + 14, frame.width - 330)),
							top: Math.min(Math.max(shown.y - 20, 6), HEIGHT - 120),
						}}
					>
						{shown.work ? (
							<>
								<div className="font-sans text-sm leading-snug text-foreground">
									<MathText>{shown.work.title}</MathText>
								</div>
								{shown.work.authors.length > 0 && <div className="mt-1 text-muted-foreground">{authorLine(shown.work)}</div>}
								<div className="mt-1 text-muted-foreground">
									{[shown.work.year ?? "year unknown", shown.work.venue].filter(Boolean).join(" · ")}
								</div>
								<div className="mt-1 text-muted-foreground">
									{citedByLine(shown.work.cited_by)} · {linkTag(shown.work.link).label}
								</div>
							</>
						) : (
							<div className="text-foreground">This paper</div>
						)}
					</div>
				)}
			</div>
			<figcaption className="flex flex-wrap items-center gap-x-4 gap-y-1 font-ui text-xs text-muted-foreground">
				<span title="Each work sits at its year along the bottom; the most cited of a year nearest the middle line.">
					← earlier · time · later →
				</span>
				<span title="The works this paper's reference list cites, at the year of each.">
					<Dot hue={HUE.prior} /> prior work: it cites
				</span>
				<span title="The papers Constellate has read whose reference lists cite this paper, at the year of each.">
					<Dot hue={HUE.derivative} /> derivative work: cites it
				</span>
				<span title="Filled: in your library (click to open it here). Ring: on arXiv. Dashed ring: not found; what was read of it is shown.">
					filled: in your library · ring: on arXiv · dashed: not found
				</span>
				{kinds && kinds.size > 0 && (
					<span title="The model's reading of the citing sentences, listed below with each sentence and the words that show it: an estimate.">
						coloured line: the kind of citation, as the model read it
					</span>
				)}
				<span title="Size is how many papers Constellate has read cite the work: only papers whose PDFs it has read are counted.">
					size: times cited by papers read
				</span>
			</figcaption>
		</figure>
	);
}

function Dot({ hue }: { hue: number }) {
	return <span className="mr-1 inline-block size-2 rounded-full align-middle" style={{ background: hsl(hue) }} />;
}

/** One list of works, as rows. */
function Works({ works, onOpen }: { works: CitationNeighbour[]; onOpen: (work: CitationNeighbour) => void }) {
	return (
		<ol className="flex flex-col">
			{works.map((work) => (
				<li key={work.key}>
					<RowButton
						className="flex-col gap-0.5"
						title={work.entry ? `As written in the reference list: ${work.entry}` : undefined}
						onClick={() => onOpen(work)}
					>
						<span className={cn("text-base leading-snug", work.confidence === "raw_only" && "text-muted-foreground")}>
							<MathText>{work.title}</MathText>
						</span>
						<span className="flex flex-wrap items-center gap-x-3 gap-y-1 font-ui text-xs text-muted-foreground">
							{work.authors.length > 0 && <span className="max-w-[28rem] truncate">{authorLine(work)}</span>}
							<span>{work.year ?? "year unknown"}</span>
							{work.venue && <span className="max-w-[20rem] truncate">{work.venue}</span>}
							<span title="Only papers whose PDFs Constellate has read are counted, not the whole literature.">
								{citedByLine(work.cited_by)}
							</span>
							<LinkTag work={work} />
						</span>
					</RowButton>
				</li>
			))}
		</ol>
	);
}

const ORDERS = [
	{ value: "cited" as const, label: "Most cited", title: "Most cited by the papers Constellate has read first" },
	{ value: "year" as const, label: "Newest", title: "Newest first; works with no year last" },
];

/** The two lists, prior and derivative work, with one order and one filter for both. */
function Lists({ network, onOpen }: { network: CitationNetwork; onOpen: (work: CitationNeighbour) => void }) {
	const [order, setOrder] = useState<WorkOrder>("cited");
	const [filter, setFilter] = useState("");
	const references = shownWorks(network.references, order, filter);
	const citations = shownWorks(network.citations, order, filter);
	const { totals } = network;
	const limited = totals.citations_shown < totals.citations;
	return (
		<div className="flex flex-col gap-4">
			<div className="flex items-center gap-2">
				<Input placeholder="Filter by title, author, venue or year" value={filter} onChange={(event) => setFilter(event.target.value)} />
				<Segmented options={ORDERS} value={order} onChange={setOrder} />
			</div>
			<section className="flex flex-col gap-1.5">
				<Heading className="font-ui">Cites: prior work</Heading>
				{network.seed_state === "read" && totals.references > 0 && (
					<p
						className="font-ui text-xs text-muted-foreground"
						title="In your library: can be opened here. On arXiv: its arXiv id is known. Not found: only what was read of the entry is known."
					>
						{showing(references.length, totals.references, "reference")}: {totals.references_in_library.toLocaleString()} in your library,{" "}
						{totals.references_on_arxiv.toLocaleString()} on arXiv, {totals.references_unresolved.toLocaleString()} not found
					</p>
				)}
				{network.seed_state === "read" && totals.references === 0 && (
					<p className="text-base text-muted-foreground">
						No reference list was found in this paper&apos;s text: it may have none, or it is laid out in a way the reader does not
						recognise.
					</p>
				)}
				<Works works={references} onOpen={onOpen} />
			</section>
			<section className="flex flex-col gap-1.5">
				<Heading className="font-ui">Cited by: derivative work</Heading>
				{totals.citations > 0 ? (
					<p className="font-ui text-xs text-muted-foreground">
						{showing(citations.length, totals.citations, "citing paper")} Constellate has read{limited ? ", the most cited" : ""}
					</p>
				) : (
					<p className="text-base text-muted-foreground">
						None of the {count(network.papers_read, "paper")} Constellate has read cite this paper. Papers that cite it but whose PDFs have
						not been read, or that are not in your library, are not counted: this is not the whole literature.
					</p>
				)}
				<Works works={citations} onOpen={onOpen} />
			</section>
		</div>
	);
}

/**
 * The Citations tab of the reader: the network around `paper`, built from the reference lists Constellate has read.
 * Mounted only when the tab is opened, so nothing is fetched before.
 */
export function Citations({ paper, run, onOpen }: { paper: Paper; run?: string; onOpen: (id: string) => void }) {
	const kept = useAsync(() => api.citations(paper.id, run), [paper.id, run], { keep: false });
	const [asked, setAsked] = useState(false);
	const reading = useAsync(asked ? () => api.readReferences(paper.id, run) : null, [asked, paper.id, run], { keep: false });
	// the first click reads; after a failure, the same button reads again
	const read = asked ? reading.reload : () => setAsked(true);
	const network = ready(reading) ?? ready(kept);
	const kinds = ready(useAsync(() => contextsOf(paper.id, run).then(edgeKinds), [paper.id, run], { keep: false }));
	const open = opener(onOpen);

	if (!network) {
		if (kept.status === "failed")
			return (
				<div className="flex flex-col items-start gap-2 font-ui">
					<p className="text-sm text-destructive">The citation network could not be loaded: {kept.error}</p>
					<Button variant="outline" onClick={kept.reload}>
						<RotateCw />
						Try again
					</Button>
				</div>
			);
		return <p className="text-sm text-muted-foreground">Loading the citation network…</p>;
	}
	const { totals } = network;
	const stopped = network.seed_state === "not_read" ? null : lookupsStoppedLine(network.lookups_stopped);
	return (
		<div className="mx-auto flex max-w-[68rem] flex-col gap-4 font-sans">
			<p
				className="font-ui text-xs text-muted-foreground"
				title="Constellate reads the reference list of each paper whose PDF it reads; nothing is asked of an outside citation service."
			>
				From the reference lists of the papers Constellate has read: {network.papers_read.toLocaleString()} of{" "}
				{count(network.papers_in_library, "paper")} in your library. Built {network.built}.
				{totals.citations_in_run !== null && ` Cited by ${count(totals.citations_in_run, "paper")} of this run.`}
			</p>
			{network.seed_state === "not_read" && (
				<div className="flex flex-col items-start gap-2 rounded-lg border p-3 font-ui">
					{paper.readable ? (
						<>
							<p className="text-xs text-muted-foreground">
								This paper&apos;s own reference list has not been read yet, so its prior work is not shown. Reading it fetches its PDF once.
							</p>
							<Button variant="outline" disabled={reading.status === "loading"} onClick={read}>
								{reading.status === "loading" ? <Loader2 className="animate-spin" /> : <BookOpenText />}
								{reading.status === "loading" ? "Reading the paper's references" : "Read this paper's references"}
							</Button>
							{reading.status === "failed" && <p className="text-xs text-destructive">{reading.error}</p>}
						</>
					) : (
						<p className="text-xs text-muted-foreground">
							This paper&apos;s PDF is not published anywhere a program may fetch it from, so its reference list cannot be read: only the
							papers that cite it are shown.
						</p>
					)}
				</div>
			)}
			{stopped && (
				<div className="flex flex-col items-start gap-2 rounded-lg border p-3 font-ui">
					<p className="text-xs text-warning">{stopped}</p>
					{paper.readable && (
						<Button variant="outline" disabled={reading.status === "loading"} onClick={read}>
							{reading.status === "loading" ? <Loader2 className="animate-spin" /> : <BookOpenText />}
							{reading.status === "loading" ? "Reading the paper's references" : "Read the references again"}
						</Button>
					)}
					{reading.status === "failed" && <p className="text-xs text-destructive">{reading.error}</p>}
				</div>
			)}
			{totals.references + totals.citations > 0 && <Picture network={network} seed={paper} onOpen={open} kinds={kinds} />}
			<Lists network={network} onOpen={open} />
		</div>
	);
}
