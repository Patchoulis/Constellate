import type { Usage, UsageEntry } from "@rp/contracts";
import { count, tokens } from "@/lib/units";

/**
 * What the sidebar says was spent: "USD 3.77 spent (USD 1.20 this run)" while a run is open, "USD 3.77 spent" when
 * none is. Both figures are read from the library (engine api/usage.rs), so they are the same after the app is closed
 * and opened again. A figure with no known price (calls to a model with none) is said in calls and tokens, never as
 * an amount: "2 calls, 1.5k tokens". `cost` says an amount in US dollars in the user's currency, with its code.
 */
export function spentLine(spent: Usage, run: string | null, cost: (usd: number) => string): string {
	const { total, ofRun } = spentParts(spent, run, cost);
	return ofRun === null ? total : `${total} ${ofRun}`;
}

/** The line's two parts, drawn apart so that the run's goes onto a line of its own in a narrow sidebar. */
export function spentParts(spent: Usage, run: string | null, cost: (usd: number) => string): { total: string; ofRun: string | null } {
	const total = spent.usd === null ? `${count(spent.calls, "call")}, ${tokens(tokensOf(spent.models))}` : `${cost(spent.usd)} spent`;
	const figure = run === null ? null : runFigure(spent, cost);
	return { total, ofRun: figure === null ? null : `(${figure} this run)` };
}

/** The open run's figure, without its words: none when the engine was not asked about a run. */
function runFigure(spent: Usage, cost: (usd: number) => string): string | null {
	if (spent.run_usd !== null) return cost(spent.run_usd);
	if (spent.run_calls === 0) return null;
	return `${count(spent.run_calls, "call")}, ${tokens(tokensOf(spent.run_purposes))}`;
}

/** Tokens sent and received, of every entry. */
function tokensOf(table: Record<string, UsageEntry>): number {
	return Object.values(table).reduce((sum, entry) => sum + entry.input_tokens + entry.output_tokens, 0);
}

/** One line per model or purpose, indented, for the tooltip; one with no priced call shows no amount (none is known). */
export function usageLines(table: Record<string, UsageEntry>, cost: (usd: number) => string): string[] {
	return Object.entries(table).map(([name, entry]) => {
		const counted = `${count(entry.calls, "call")}, ${tokens(entry.input_tokens)} sent, ${tokens(entry.output_tokens)} received`;
		return entry.usd === null ? `  ${name}: ${counted}` : `  ${name}: ${cost(entry.usd)} (${counted})`;
	});
}

/** What the tooltip says of the open run (`title` as the list shows it), after the breakdown of everything spent. */
export function runDetail(spent: Usage, run: string | null, title: string, cost: (usd: number) => string): string[] {
	const figure = run === null ? null : runFigure(spent, cost);
	if (figure === null) return [];
	return [
		`This run, ${title}: ${figure}${spent.run_usd === null ? "" : `, ${count(spent.run_calls, "call")}`}.`,
		...(spent.run_calls ? ["This run by purpose:", ...usageLines(spent.run_purposes, cost)] : []),
		...(spent.run_unpriced_calls && spent.run_usd !== null
			? [
					`${count(spent.run_unpriced_calls, "call")} of this run to a model with no price ${spent.run_unpriced_calls === 1 ? "is" : "are"} not in its figure.`,
				]
			: []),
		...(spent.run_counted_since
			? [`This run's figure is counted since ${spent.run_counted_since}: what was spent on it before then is in the total only.`]
			: []),
		"Deleting a run does not take what it cost off the total; a run made again under the same name goes on counting.",
	];
}
