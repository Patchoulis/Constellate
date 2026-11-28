/**
 * Naming a made run's communities, which asks the language model and so costs money: what the run has and what
 * asking would cost are shown first (`GET /api/runs/<run>/describe/estimate`), and the job is started with what
 * was shown (`POST /api/runs/<run>/describe`). Nothing here sends anything.
 *
 * Three requests share that flow (`NamingMode`): naming as usual ("name", which never asks again about a community
 * described plainly before sections were asked for), rewriting those plain descriptions with sections and
 * citations ("sections"), and describing all again ("again"). Each is shown its own estimate.
 */
import type { Community, CommunitySection, DescribeEstimate, DescribeRequest } from "@rp/contracts";
import { accepting, costWords, shownModel } from "@/lib/cost";
import { count, type Currency } from "@/lib/units";

/** What a naming request asks for: name as usual, rewrite the plain descriptions with sections, or all again. */
export type NamingMode = "name" | "sections" | "again";

/** How many of a run's communities and sub-communities have no name. */
export function unnamedOf(communities: Community[]): { communities: number; unnamed: number } {
	let all = 0;
	let unnamed = 0;
	const walk = (list: Community[]) => {
		for (const community of list) {
			all += 1;
			if (!community.name) unnamed += 1;
			walk(community.children);
		}
	};
	walk(communities);
	return { communities: all, unnamed };
}

/** Is it named with a plain description only: described before sections were asked for? */
export function isPlain(community: Community): boolean {
	return Boolean(community.name?.trim()) && community.sections === undefined;
}

/** How many of a run's communities and sub-communities have a plain description and no sections. */
export function plainOf(communities: Community[]): { communities: number; plain: number } {
	let all = 0;
	let plain = 0;
	const walk = (list: Community[]) => {
		for (const community of list) {
			all += 1;
			if (isPlain(community)) plain += 1;
			walk(community.children);
		}
	};
	walk(communities);
	return { communities: all, plain };
}

/** Its sections to show, or null when it has none to show (a plain description, or none written). */
export function sectionsOf(community: Community): CommunitySection[] | null {
	return community.sections && community.sections.length > 0 ? community.sections : null;
}

/** "1 section left out: its quotes were not found in the papers", or null when none was. */
export function leftOutWords(leftOut: number | undefined): string | null {
	if (!leftOut) return null;
	return leftOut === 1
		? "1 section left out: its quotes were not found in the papers."
		: `${leftOut.toLocaleString("en-US")} sections left out: their quotes were not found in the papers.`;
}

/** The query of the estimate a mode is shown. */
export function estimateQuery(mode: NamingMode): string {
	return mode === "sections" ? "?add_sections=true" : mode === "again" ? "?redo=true" : "";
}

export interface NamingWords {
	/** "24 communities, 12 of them named." */
	counted: string;
	/** "12 to ask a-model about (3 kept from before cost nothing)", or that nothing needs asking */
	asked: string;
	/** "about USD 0.12, at most USD 0.25", "at most 38k tokens"; null when nothing is asked */
	cost: string | null;
	/** nothing is to be asked: no paid click is offered */
	nothing: boolean;
}

/** What the naming dialog says of an estimate, for the request it is for. */
export function namingWords(estimate: DescribeEstimate, currency?: Currency, locale?: string, mode: NamingMode = "name"): NamingWords {
	const { communities, named } = estimate;
	const batch = estimate.estimate;
	if (mode === "sections") return sectionsWords(estimate, currency, locale);
	const counted = `${count(communities, "community", "communities")}, ${named === 0 ? "none" : named.toLocaleString("en-US")} of them named.`;
	if (batch.to_ask === 0) {
		return { counted, asked: "Every one has a name kept for this model: nothing needs asking.", cost: null, nothing: true };
	}
	const kept = batch.kept > 0 ? ` (${count(batch.kept, "name")} kept from before ${batch.kept === 1 ? "costs" : "cost"} nothing)` : "";
	return {
		counted,
		asked: `${count(batch.to_ask, "community", "communities")} to name with ${estimate.model}${kept}.`,
		cost: costWords(batch, currency, locale),
		nothing: false,
	};
}

/** What the dialog rewriting plain descriptions with sections says: how many are plain, how many calls, the cost. */
function sectionsWords(estimate: DescribeEstimate, currency?: Currency, locale?: string): NamingWords {
	const { communities, plain } = estimate;
	const batch = estimate.estimate;
	const counted = `${count(communities, "community", "communities")}, ${plain === 0 ? "none" : plain.toLocaleString("en-US")} of them with a plain description and no sections.`;
	if (batch.to_ask === 0) {
		return { counted, asked: "Nothing needs asking: every description kept for this model has its sections.", cost: null, nothing: true };
	}
	const kept = batch.kept > 0 ? ` (${count(batch.kept, "answer")} kept from before ${batch.kept === 1 ? "costs" : "cost"} nothing)` : "";
	return {
		counted,
		asked:
			`${count(batch.to_ask, "call")} to ${estimate.model}, each about one community or the two parts of one${kept}. ` +
			"The communities they are about may be renamed.",
		cost: costWords(batch, currency, locale),
		nothing: false,
	};
}

/** The request that names them on the terms shown: the model and the most of its estimate, and what is asked. */
export function namingRequest(estimate: DescribeEstimate, mode: NamingMode): DescribeRequest {
	const asked = mode === "again" ? { redo: true } : mode === "sections" ? { add_sections: true } : {};
	return { ...shownModel(estimate.estimate), ...accepting(estimate.estimate), ...asked };
}
