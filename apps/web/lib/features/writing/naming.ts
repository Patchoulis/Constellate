/**
 * Naming a made run's communities, which asks the language model and so costs money: what the run has and what
 * asking would cost are shown first (`GET /api/runs/<run>/describe/estimate`), and the job is started with what
 * was shown (`POST /api/runs/<run>/describe`). Nothing here sends anything.
 */
import type { Community, DescribeEstimate, DescribeRequest } from "@rp/contracts";
import { accepting, costWords, shownModel } from "@/lib/cost";
import { count, type Currency } from "@/lib/units";

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

/** What the naming dialog says of an estimate. */
export function namingWords(estimate: DescribeEstimate, currency?: Currency, locale?: string): NamingWords {
	const { communities, named } = estimate;
	const batch = estimate.estimate;
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

/** The request that names them on the terms shown: the model and the most of its estimate. */
export function namingRequest(estimate: DescribeEstimate, redo: boolean): DescribeRequest {
	return { ...shownModel(estimate.estimate), ...accepting(estimate.estimate), ...(redo ? { redo: true } : {}) };
}
