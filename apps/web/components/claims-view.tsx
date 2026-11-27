"use client";

/**
 * The paper reader's Claims tab (engine analysis/claim_tree): the paper's claims as a tree, each with the paper's
 * own words, and its reviewers' points placed on them. Reviewers come first; the model's own reading of gaps
 * comes last, collapsed and labelled. Nothing is asked of the model until the user has seen the estimate and
 * clicked; nothing is shown when the engine has no such route (the Python backend).
 */
import { useEffect, useMemo, useRef, useState } from "react";
import { ChevronDown, ChevronRight, ListTree, Loader2, MessagesSquare, RotateCcw, Square } from "lucide-react";
import type {
	ClaimNode,
	ClaimNodeReviews,
	ClaimReviewer,
	ClaimReviews,
	ClaimsEstimate,
	ClaimsView as View,
	PointMapping,
	PointPlacement,
	ReviewPoint,
	Status,
} from "@rp/contracts";
import { EvidenceLink, EvidenceLinks } from "@/components/evidence-link";
import { REVIEWS_NOTE } from "@/components/review-summary";
import { Button, Heading, Segmented } from "@/components/ui/button";
import { api } from "@/lib/api";
import {
	ESTIMATE_NOTE,
	MAPPING_NOTE,
	MODEL_NOTE,
	allOpen,
	budgetFor,
	byReviewers,
	estimateLine,
	kindLabel,
	leftOutLine,
	marked,
	move,
	outline,
	pointsAt,
	ratingLabel,
	ratingNote,
	reviewsAt,
	stanceTone,
	stanceWords,
	unplacedWhy,
} from "@/lib/claim-tree";
import { ready, useAsync } from "@/lib/use-async";
import { count, money, onScale, scaleName, spreadPoints } from "@/lib/units";
import { cn } from "@/lib/utils";

const POLL_MS = 1200;

/** The asking job, as this view follows it. */
type Asking =
	{ state: "idle" } | { state: "starting" } | { state: "running"; job: number; status?: Status } | { state: "failed"; error: string };

function Rating({ reviewer }: { reviewer: ClaimReviewer | undefined }) {
	if (!reviewer?.overall) return null;
	return (
		<span className="text-muted-foreground tabular-nums" title={`${ratingNote(reviewer.overall)} ${REVIEWS_NOTE}`}>
			{" "}
			({ratingLabel(reviewer.overall)})
		</span>
	);
}

/** The estimate, the button that starts the work, its progress and its stop button. */
function Ask({ id, what, estimate, onDone }: { id: string; what: string; estimate: ClaimsEstimate; onDone: () => void }) {
	const [asking, setAsking] = useState<Asking>({ state: "idle" });
	const done = useRef(onDone);
	done.current = onDone;
	const budget = budgetFor(estimate);
	useEffect(() => {
		if (asking.state !== "running") return;
		const timer = setTimeout(async () => {
			try {
				const status = await api.status(1_000_000);
				if (status.id !== asking.job || status.state !== "running") {
					if (status.id === asking.job && status.state === "failed")
						setAsking({ state: "failed", error: status.error ?? "The work failed." });
					else setAsking({ state: "idle" });
					done.current();
					return;
				}
				setAsking({ ...asking, status });
			} catch (error) {
				setAsking({ state: "failed", error: (error as Error).message });
			}
		}, POLL_MS);
		return () => clearTimeout(timer);
	}, [asking]);
	const start = async () => {
		if (budget === null) return;
		setAsking({ state: "starting" });
		try {
			const { job } = await api.startClaims(id, { budget_usd: budget });
			setAsking({ state: "running", job });
		} catch (error) {
			setAsking({ state: "failed", error: (error as Error).message });
		}
	};
	const progress = asking.state === "running" ? asking.status?.progress : undefined;
	return (
		<div className="flex flex-col items-start gap-1.5 rounded-md border px-3 py-2 font-ui text-xs">
			<span className="text-foreground">{what}</span>
			<span className="text-muted-foreground tabular-nums" title={ESTIMATE_NOTE}>
				{estimateLine(estimate)}
				{estimate.by_model[0] && `, with ${estimate.by_model[0].model}`}
			</span>
			{asking.state === "running" ? (
				<div className="flex items-center gap-2">
					<Loader2 className="size-3.5 animate-spin" />
					<span className="tabular-nums">
						{progress
							? `${progress.done} of ${count(progress.of, "call")} done, ${money(progress.usd, { code: "USD", rate: 1, date: null })} so far`
							: "Asking the model"}
					</span>
					<Button variant="ghost" size="icon" title="Stop: nothing new is asked" onClick={() => void api.cancelJob(asking.job)}>
						<Square />
					</Button>
				</div>
			) : (
				<Button
					variant="outline"
					disabled={asking.state === "starting" || budget === null || estimate.to_ask === 0}
					title={
						budget === null
							? "The model's price is not known, so no budget can be set."
							: `At most ${money(budget, { code: "USD", rate: 1, date: null })} is spent.`
					}
					onClick={start}
				>
					{asking.state === "starting" ? <Loader2 className="animate-spin" /> : <ListTree />}
					Ask the model
				</Button>
			)}
			{asking.state === "failed" && <span className="text-destructive">{asking.error}</span>}
		</div>
	);
}

/** One reviewer's point under a node: stance in words and colour, the reviewer, and on expand both texts side by side. */
function PointRow({
	point,
	placement,
	node,
	reviewer,
	onOpenReviews,
}: {
	point: ReviewPoint;
	placement: PointPlacement;
	node: ClaimNode | null;
	reviewer: ClaimReviewer | undefined;
	onOpenReviews?: () => void;
}) {
	const [open, setOpen] = useState(false);
	const [before, words, after] = marked(point, placement);
	return (
		<li className="flex flex-col gap-1">
			<button
				type="button"
				className="flex w-full items-baseline gap-2 rounded-md px-2 py-1 text-left hover:bg-accent"
				aria-expanded={open}
				onClick={() => setOpen(!open)}
				title={MAPPING_NOTE}
			>
				{open ? <ChevronDown className="size-3 shrink-0" /> : <ChevronRight className="size-3 shrink-0" />}
				<span className={cn("shrink-0 font-ui text-xs font-semibold", stanceTone(placement.stance))}>{stanceWords(placement.stance)}</span>
				<span className="min-w-0 flex-1 truncate font-sans text-sm">{point.text}</span>
				<span className="shrink-0 font-ui text-xs text-muted-foreground">
					{point.reviewer}
					<Rating reviewer={reviewer} />
				</span>
			</button>
			{open && (
				<div className="ml-5 grid grid-cols-1 gap-3 rounded-md border p-2 md:grid-cols-2">
					<div className="flex flex-col gap-1">
						<Heading>{node ? "The paper" : "The paper as a whole"}</Heading>
						{node ? (
							<>
								<p className="font-sans text-sm" title={MODEL_NOTE}>
									{node.statement}
								</p>
								{node.evidence.map((piece, index) => (
									<blockquote key={index} className="border-l-2 pl-2 font-sans text-sm text-foreground/80">
										&ldquo;{piece.quote}&rdquo; <EvidenceLink evidence={piece} />
									</blockquote>
								))}
							</>
						) : (
							<p className="font-sans text-sm text-muted-foreground">The point is about the paper as a whole, not one claim.</p>
						)}
					</div>
					<div className="flex flex-col gap-1">
						<Heading>
							{point.reviewer}, {point.section}
						</Heading>
						<p
							className="font-sans text-sm whitespace-pre-wrap"
							title="The reviewer's own words, as the reviews page you opened has them; the marked words are those the model gave as the reason for the placement."
						>
							{before}
							<mark className="rounded-sm bg-primary/25 text-foreground">{words}</mark>
							{after}
						</p>
						{placement.matched.kind === "close" && (
							<span className="font-ui text-xs text-muted-foreground">
								The marked words differ from the model&apos;s by {count(placement.matched.differences, "letter")}.
							</span>
						)}
						{onOpenReviews && (
							<button
								type="button"
								className="self-start font-ui text-xs text-primary underline-offset-2 hover:underline"
								onClick={onOpenReviews}
							>
								<MessagesSquare className="mr-1 inline size-3" />
								Open the reviews page
							</button>
						)}
						{reviewer && reviewer.replies.length > 0 && (
							<div className="mt-1 flex flex-col gap-1 border-l pl-2">
								{reviewer.replies.map((reply) => (
									<div key={reply.note} className="flex flex-col">
										<span className="font-ui text-xs text-muted-foreground">
											{reply.heading} ·{" "}
											{reply.attached.by === "named"
												? "a reply to this review, as its title says"
												: `guessed to reply to this review (${
														{
															same_reviewer: "by the same reviewer",
															mentions_reviewer: "it names the reviewer",
															follows_review: "it follows the review",
														}[reply.attached.basis]
													})`}
										</span>
										{reply.sections.map((section, index) => (
											<p key={index} className="font-sans text-sm whitespace-pre-wrap text-foreground/80">
												{section.text}
											</p>
										))}
									</div>
								))}
							</div>
						)}
					</div>
				</div>
			)}
		</li>
	);
}

/** "challenged by 2 of 4 reviewers (Reviewer aBcD, 4 of 10 rating; ...)" for each stance that has any. */
function Tally({ here, reviewers }: { here: ClaimNodeReviews | undefined; reviewers: ClaimReviewer[] }) {
	if (!here) return null;
	const byName = (name: string) => reviewers.find((reviewer) => reviewer.name === name);
	const parts: [string, string[], string][] = [
		["challenged", here.reviewers.challenges, "text-warning"],
		["supported", here.reviewers.supports, "text-success"],
		["asked about", here.reviewers.asks, "text-primary"],
	];
	return (
		<span className="flex flex-wrap gap-x-3 font-ui text-xs">
			{parts
				.filter(([, names]) => names.length > 0)
				.map(([verb, names, tone]) => (
					<span key={verb} className={tone} title={`${names.join(", ")}. ${MAPPING_NOTE}`}>
						{byReviewers(verb, names, reviewers.length)}
						<span className="text-muted-foreground">
							{" "}
							(
							{names.map((name, index) => (
								<span key={name}>
									{index > 0 && "; "}
									{name}
									<Rating reviewer={byName(name)} />
								</span>
							))}
							)
						</span>
					</span>
				))}
		</span>
	);
}

/** The root: the paper's figures, the meta-review and decision, and the points about the paper as a whole. */
function Root({ reviews, mapping, onOpenReviews }: { reviews: ClaimReviews; mapping: PointMapping | null; onOpenReviews?: () => void }) {
	const { figures } = reviews;
	const overall = figures.fields.find((field) => field.field !== "Confidence");
	const disagreement = figures.disagreement;
	const [meta, setMeta] = useState(false);
	const whole = mapping ? pointsAt(null, mapping, reviews.points) : [];
	return (
		<section className="flex flex-col gap-1.5 rounded-md border px-3 py-2">
			<Heading>The paper as a whole</Heading>
			<div className="flex flex-wrap gap-x-4 gap-y-1 font-ui text-xs text-muted-foreground tabular-nums" title={REVIEWS_NOTE}>
				<span>{count(reviews.reviewers.length, "reviewer")}</span>
				{overall && (
					<span title={`On ${scaleName(overall.scale)}${overall.scale.source === "assumed" ? ", assumed" : ""}. ${REVIEWS_NOTE}`}>
						{overall.field}: {onScale(overall.mean, overall.scale, `mean ${overall.field.toLowerCase()}`)}, range {overall.minimum} to{" "}
						{onScale(overall.maximum, overall.scale, "").trim()}, {count(overall.reviews, "review")}
					</span>
				)}
				{disagreement && (
					<span>
						reviewers disagreed by {spreadPoints(disagreement.spread)} on {scaleName(disagreement.scale)}
					</span>
				)}
				{reviews.decision && <span className="text-foreground">Decision: {reviews.decision}</span>}
			</div>
			{reviews.meta_reviews.length > 0 && (
				<div>
					<button
						type="button"
						className="font-ui text-xs text-muted-foreground hover:text-foreground"
						aria-expanded={meta}
						onClick={() => setMeta(!meta)}
					>
						{meta ? <ChevronDown className="inline size-3" /> : <ChevronRight className="inline size-3" />} The meta-review
					</button>
					{meta &&
						reviews.meta_reviews.map((review, index) => (
							<div key={index} className="ml-4 flex flex-col gap-1">
								{review.sections.map((section, at) => (
									<p key={at} className="font-sans text-sm whitespace-pre-wrap">
										{section.name && <span className="font-ui text-xs text-muted-foreground">{section.name}: </span>}
										{section.text}
									</p>
								))}
							</div>
						))}
				</div>
			)}
			{whole.length > 0 && (
				<ul className="flex flex-col">
					{whole.map(({ point, placement }) => (
						<PointRow
							key={point.id}
							point={point}
							placement={placement}
							node={null}
							reviewer={reviews.reviewers.find((reviewer) => reviewer.note === point.review)}
							onOpenReviews={onOpenReviews}
						/>
					))}
				</ul>
			)}
		</section>
	);
}

/** The reviews as stored, by reviewer: for a paper with no tree. */
function StoredReviews({ reviews }: { reviews: ClaimReviews }) {
	return (
		<section className="flex flex-col gap-3">
			{reviews.reviewers.map((reviewer) => (
				<div key={reviewer.note} className="flex flex-col gap-1">
					<Heading>
						{reviewer.name}
						<Rating reviewer={reviewer} />
					</Heading>
					{reviews.points
						.filter((point) => point.review === reviewer.note)
						.map((point) => (
							<p key={point.id} className="font-sans text-sm">
								<span className="font-ui text-xs text-muted-foreground">{point.section}: </span>
								{point.text}
							</p>
						))}
				</div>
			))}
		</section>
	);
}

/** The tree, as an outline you can move through with the arrow keys. */
function Outline({ view, nodes, onOpenReviews }: { view: View; nodes: ClaimNode[]; onOpenReviews?: () => void }) {
	const [open, setOpen] = useState<Set<string>>(() => allOpen(nodes));
	const [onlyContested, setOnlyContested] = useState(false);
	const [focused, setFocused] = useState<string | null>(nodes[0]?.id ?? null);
	const refs = useRef(new Map<string, HTMLDivElement>());
	const rows = useMemo(() => outline(nodes, view.node_reviews, open, onlyContested), [nodes, view.node_reviews, open, onlyContested]);
	// the row that takes the keyboard: the one last focused while it is shown, else the first, so that Tab always reaches the tree
	const current = rows.some((row) => row.node.id === focused) ? focused : (rows[0]?.node.id ?? null);
	const mapping = view.mapping.state === "made" ? view.mapping.mapping : null;
	const reviews = view.reviews;
	const reviewers = reviews?.reviewers ?? [];
	const toggle = (id: string) =>
		setOpen((before) => {
			const next = new Set(before);
			if (next.has(id)) next.delete(id);
			else next.add(id);
			return next;
		});
	const onKey = (event: React.KeyboardEvent) => {
		// keys pressed on a button inside a row (a point, a citation) are that button's
		if ((event.target as HTMLElement).getAttribute("role") !== "treeitem") return;
		const moved = move(rows, current, event.key);
		if (!moved) return;
		event.preventDefault();
		if ("toggle" in moved) toggle(moved.toggle);
		else {
			setFocused(moved.focus);
			refs.current.get(moved.focus)?.focus();
		}
	};
	return (
		<section className="flex flex-col gap-2">
			<div className="flex items-center gap-2">
				<Heading className="flex-1">The paper&apos;s claims</Heading>
				{mapping && (
					<Segmented
						options={[
							{ value: "all", label: "All claims" },
							{
								value: "contested",
								label: `Only contested (${count(view.contested, "claim")})`,
								title: "Claims at least one reviewer challenged, and what they stand under",
							},
						]}
						value={onlyContested ? "contested" : "all"}
						onChange={(value) => setOnlyContested(value === "contested")}
					/>
				)}
			</div>
			<div role="tree" aria-label="The paper's claims" className="flex flex-col" onKeyDown={onKey}>
				{rows.map((row) => {
					const here = reviewsAt(view.node_reviews, row.node.id);
					const placed = mapping && reviews ? pointsAt(row.node.id, mapping, reviews.points) : [];
					return (
						<div
							key={row.node.id}
							ref={(element) => {
								if (element) refs.current.set(row.node.id, element);
								else refs.current.delete(row.node.id);
							}}
							role="treeitem"
							aria-level={row.depth}
							aria-expanded={row.hasChildren ? row.open : undefined}
							aria-selected={current === row.node.id}
							tabIndex={current === row.node.id ? 0 : -1}
							onFocus={() => setFocused(row.node.id)}
							className={cn(
								"flex flex-col gap-1 rounded-md py-1.5 pr-2 outline-none focus-visible:ring-1 focus-visible:ring-ring",
								row.contested && "border-l-2 border-warning bg-warning/5",
							)}
							style={{ paddingLeft: `${(row.depth - 1) * 1.25 + 0.5}rem` }}
						>
							<div className="flex items-baseline gap-2">
								{row.hasChildren ? (
									<button
										type="button"
										tabIndex={-1}
										className="text-muted-foreground"
										aria-label={row.open ? "Close" : "Open"}
										onClick={() => toggle(row.node.id)}
									>
										{row.open ? <ChevronDown className="size-3" /> : <ChevronRight className="size-3" />}
									</button>
								) : (
									<span className="w-3 shrink-0" />
								)}
								<span className="shrink-0 font-ui text-xs text-muted-foreground uppercase">{kindLabel(row.node)}</span>
								{row.contested && (
									<span className="shrink-0 font-ui text-xs font-semibold text-warning" title="At least one reviewer challenged this.">
										Contested
									</span>
								)}
							</div>
							<p className="ml-5 font-sans text-sm leading-snug" title={MODEL_NOTE}>
								{row.node.statement} <EvidenceLinks evidence={row.node.evidence} />
								{row.node.points_at && (
									<span className="font-ui text-xs text-muted-foreground">
										{" "}
										· in the paper: <EvidenceLink evidence={row.node.points_at} />
									</span>
								)}
							</p>
							<div className="ml-5">
								<Tally here={here} reviewers={reviewers} />
							</div>
							{placed.length > 0 && reviews && (
								<ul className="ml-4 flex flex-col">
									{placed.map(({ point, placement }) => (
										<PointRow
											key={point.id}
											point={point}
											placement={placement}
											node={row.node}
											reviewer={reviewers.find((reviewer) => reviewer.note === point.review)}
											onOpenReviews={onOpenReviews}
										/>
									))}
								</ul>
							)}
						</div>
					);
				})}
				{rows.length === 0 && (
					<p className="font-ui text-xs text-muted-foreground">
						{nodes.length === 0
							? "Nothing of the model's answer is shown: none of its statements' quotes were found in the paper."
							: "No claim was challenged by a reviewer."}
					</p>
				)}
			</div>
		</section>
	);
}

/** Claims and findings with no evidence found under them, and evidence no reviewer examined: the model's reading. */
function Gaps({ view, nodes }: { view: View; nodes: ClaimNode[] }) {
	if (view.gaps.length === 0) return null;
	return (
		<details className="rounded-md border border-dashed px-3 py-2">
			<summary
				className="cursor-pointer font-ui text-xs text-muted-foreground"
				title="Found by a program from the model's tree, not by a reviewer: an estimate of what may be missing, with no score and no verdict."
			>
				Possible gaps, the model&apos;s reading ({count(view.gaps.length, "item")})
			</summary>
			<ul className="mt-2 flex flex-col gap-2">
				{view.gaps.map((gap) => {
					const node = nodes.find((candidate) => candidate.id === gap.node);
					return (
						<li key={`${gap.kind}:${gap.node}`} className="flex flex-col gap-0.5">
							<span className="font-ui text-xs text-muted-foreground">
								{gap.kind === "no_stated_support" ? "Claim with no stated support found" : "Evidence no reviewer's point was placed on"}
								{node && ` · ${kindLabel(node)}`}
							</span>
							<p className="font-sans text-sm" title={MODEL_NOTE}>
								{gap.statement} <EvidenceLinks evidence={gap.evidence} />
							</p>
						</li>
					);
				})}
			</ul>
		</details>
	);
}

/** The Claims tab of one paper. `onOpenReviews` opens its Reviews tab, where its reviews are read. */
export function ClaimsView({ id, onOpenReviews }: { id: string; onOpenReviews?: () => void }) {
	const loaded = useAsync(() => api.claims(id), [id]);
	const view = ready(loaded);
	if (!view) {
		if (loaded.status === "loading") return <p className="font-ui text-sm text-muted-foreground">Loading…</p>;
		return null; // an engine without this route (or a paper it does not have): nothing to show
	}
	const reviews = view.reviews;
	const tree = view.tree;
	const mapping = view.mapping.state === "made" ? view.mapping.mapping : null;
	const nodes = tree.state === "made" ? tree.nodes : [];
	const leftOut = tree.state === "made" ? leftOutLine(tree.left_out) : null;
	const redo = async () => {
		await api.forgetClaims(id).catch(() => null);
		loaded.reload();
	};
	return (
		<div className="mx-auto flex max-w-[80ch] flex-col gap-4 font-sans">
			{tree.state === "no_text" && (
				<>
					<p className="font-ui text-sm text-muted-foreground">
						No claim tree: this paper&apos;s PDF {view.paper.state === "unreadable" ? "could not be read" : "is not kept"}, so there is no
						text to find its claims in.
					</p>
					{reviews && <StoredReviews reviews={reviews} />}
				</>
			)}
			{tree.state === "not_asked" && view.estimate && (
				<Ask
					id={id}
					what={
						reviews && reviews.points.length > 0
							? `Find this paper's claims, then place ${count(reviews.points.length, "reviewer's point", "reviewers' points")} on them`
							: "Find this paper's claims and the evidence it offers for each"
					}
					estimate={view.estimate}
					onDone={loaded.reload}
				/>
			)}
			{tree.state === "malformed" && (
				<p className="font-ui text-sm text-muted-foreground">
					The model&apos;s answer was not a tree of claims, so nothing of it is shown. Ask again with the button below.
				</p>
			)}
			{reviews && tree.state === "made" && <Root reviews={reviews} mapping={mapping} onOpenReviews={onOpenReviews} />}
			{view.mapping.state === "no_reviews" && tree.state !== "no_text" && (
				<p className="font-ui text-xs text-muted-foreground">
					No reviews of this paper are kept. Open its Reviews tab once to read them; they are then placed on these claims.
					{onOpenReviews && (
						<button type="button" className="ml-1 text-primary underline-offset-2 hover:underline" onClick={onOpenReviews}>
							Open the Reviews tab
						</button>
					)}
				</p>
			)}
			{view.mapping.state === "not_asked" && view.estimate && (
				<Ask
					id={id}
					what={`Place ${count(reviews?.points.length ?? 0, "reviewer's point", "reviewers' points")} on these claims`}
					estimate={view.estimate}
					onDone={loaded.reload}
				/>
			)}
			{tree.state === "made" && <Outline view={view} nodes={nodes} onOpenReviews={onOpenReviews} />}
			{leftOut && <p className="font-ui text-xs text-muted-foreground">{leftOut}.</p>}
			{mapping && reviews && mapping.unplaced.length > 0 && (
				<section className="flex flex-col gap-1">
					<Heading>Points not placed on a claim ({count(mapping.unplaced.length, "point")})</Heading>
					{mapping.unplaced.map((unplaced) => {
						const point = reviews.points.find((candidate) => candidate.id === unplaced.point);
						return (
							point && (
								<p key={point.id} className="font-sans text-sm">
									<span className="font-ui text-xs text-muted-foreground">
										{point.reviewer}, {point.section} ({unplacedWhy(unplaced.why)}):{" "}
									</span>
									{point.text}
								</p>
							)
						);
					})}
				</section>
			)}
			{reviews && reviews.left_out > 0 && (
				<p className="font-ui text-xs text-muted-foreground">
					{count(reviews.left_out, "point")} of these reviews past the most one paper&apos;s mapping takes: not placed.
				</p>
			)}
			{tree.state === "made" && <Gaps view={view} nodes={nodes} />}
			{(tree.state === "made" || tree.state === "malformed") && (
				<div className="flex items-center gap-2 font-ui text-xs text-muted-foreground">
					<span title={MODEL_NOTE}>
						Read by {tree.state === "made" ? tree.model : "the model"} from{" "}
						{view.paper.state === "read" ? `${view.paper.sent} of ${count(view.paper.passages, "passage")}` : "the paper"}
					</span>
					<Button
						variant="ghost"
						title="Forget this paper's tree and placements; they are asked again, at a cost, only when you click to."
						onClick={redo}
					>
						<RotateCcw />
						Redo
					</Button>
				</div>
			)}
		</div>
	);
}
