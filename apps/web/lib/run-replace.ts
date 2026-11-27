/**
 * Starting a run that would make an existing one again in its folder (a "redo", the same selection anew), and
 * naming its communities, which costs money. The engine refuses both unless the person was told and agreed
 * (engine api/runs.rs): a run that exists is made again only with `replace: true` (else `409` with what would be
 * replaced, `RunReplaced`), and descriptions are asked again only with `describe`, what was accepted of the
 * estimate shown (else `400`). This file decides what is asked and what is sent; the dialog that asks is
 * components/ui/confirm.tsx, the words it says are below.
 *
 *     send(request) --started------------------------------------------> started
 *     send(request) --409 replaces--> ask(replaced, naming) --cancel----> cancelled (nothing sent again)
 *                                                           --go--------> send(request + replace, naming as chosen)
 *     a request that names nothing yet must first be asked about naming, unless a 409 will ask it (a redo)
 */
import type { DescribeBeforeRun, RedoStep, RunReplaced, RunRequest, Settings } from "@rp/contracts";
import { accepting, shownModel } from "@/lib/cost";
import { RequestError } from "@/lib/request";
import { count } from "@/lib/units";

/** What a 409 of `POST /api/runs` says would be replaced, or null for any other refusal (a job running, ...). */
export function replacedOf(error: unknown): RunReplaced | null {
	if (!(error instanceof RequestError) || error.problem.kind !== "refused" || error.status !== 409) return null;
	const body = error.problem.body as { replaces?: RunReplaced } | null | undefined;
	const replaces = body?.replaces;
	return replaces && typeof replaces === "object" && typeof replaces.run === "string" ? replaces : null;
}

/** Whether a request redoes the descriptions without having accepted what that costs. */
export function needsNaming(request: RunRequest): boolean {
	return (request.redo ?? []).includes("descriptions") && !request.describe;
}

/** What can be said about naming the communities of the run asked for. */
export type NamingOffer =
	/** naming can be paid for: the estimate before the run, to be shown and accepted */
	| { kind: "offer"; plan: DescribeBeforeRun }
	/** no key for the chosen provider: the names are kept as they are */
	| { kind: "unavailable" };

/** What the person is asked: what would be replaced (null for a run that does not exist yet), and about naming
 *  (null when the request has settled it). */
export interface Asking {
	replaced: RunReplaced | null;
	naming: NamingOffer | null;
}

/** "named": go ahead and name the communities at the cost shown; "unnamed": go ahead, the names as they are. */
export type Answer = "named" | "unnamed" | null;

export type Started = { kind: "started"; namesKept: boolean } | { kind: "cancelled" };

export interface StartDeps {
	send: (request: RunRequest) => Promise<unknown>;
	beforeRun: () => Promise<DescribeBeforeRun>;
	ask: (asking: Asking) => Promise<Answer>;
}

const without = (redo: RedoStep[] | undefined, step: RedoStep) => (redo ?? []).filter((one) => one !== step);

/** The request as the person chose: naming at the cost shown, or the descriptions left as they are. */
export function chosenRequest(request: RunRequest, naming: NamingOffer | null, answer: Exclude<Answer, null>): RunRequest {
	if (naming === null) return request;
	if (answer === "named" && naming.kind === "offer") {
		return { ...request, describe: { ...shownModel(naming.plan.estimate), ...accepting(naming.plan.estimate) } };
	}
	return { ...request, redo: without(request.redo, "descriptions") };
}

/**
 * Send a run request so that nothing that costs money or replaces a run is done without the person's click: what
 * would be replaced and what naming would cost are asked first (`ask`), and on "go" the same request is sent
 * again with `replace: true` and the naming chosen. Throws what the engine refused otherwise.
 */
export async function startRun(request: RunRequest, deps: StartDeps): Promise<Started> {
	let naming: NamingOffer | null = null;
	if (needsNaming(request)) {
		const plan = await deps.beforeRun();
		naming = plan.api_key_set ? { kind: "offer", plan } : { kind: "unavailable" };
	}
	let asked = request;
	// a request for a run not made yet has no 409 to ask about naming in: it is asked now
	if (naming !== null && !("rerun" in request)) {
		const answer = await deps.ask({ replaced: null, naming });
		if (answer === null) return { kind: "cancelled" };
		asked = chosenRequest(request, naming, answer);
	}
	const namesKept = naming !== null && !asked.describe;
	try {
		await deps.send(asked);
		return { kind: "started", namesKept };
	} catch (error) {
		const replaced = replacedOf(error);
		if (!replaced) throw error;
		const still = asked === request ? naming : null; // asked already: not asked twice
		const answer = await deps.ask({ replaced, naming: still });
		if (answer === null) return { kind: "cancelled" };
		const again: RunRequest = { ...chosenRequest(asked, still, answer), replace: true };
		await deps.send(again);
		return { kind: "started", namesKept: naming !== null && !again.describe };
	}
}

// ---- what the dialog says ----

/** A setting's value in words: on/off, a list, "not recorded" for a run that did not record it. */
export function valueWords(value: unknown, labelOf?: (value: string) => string | undefined): string {
	if (value === null || value === undefined) return "not recorded";
	if (typeof value === "boolean") return value ? "on" : "off";
	if (Array.isArray(value)) return value.length === 0 ? "none" : value.map((one) => valueWords(one, labelOf)).join(", ");
	if (typeof value === "number") return value.toLocaleString("en-US");
	if (typeof value === "string") return labelOf?.(value) ?? value;
	return JSON.stringify(value);
}

export interface ReplaceWords {
	title: string;
	/** "24 communities, 24 of them named." */
	counted: string;
	/** whether they will or may change, and why */
	will: string;
	/** "Clustering method: fiedler → spectral", one per setting that differs */
	changes: string[];
	/** what no longer applies once they change */
	consequence: string;
}

/**
 * What the dialog says before a run is made again in place. `settingLabel` names a setting as the settings dialog
 * does; `valueLabel` names a value as the menus do ("Spectral" for "spectral"), when they have a label for it.
 */
export function replaceWords(
	replaced: RunReplaced,
	settingLabel: (setting: keyof Settings) => string,
	valueLabel?: (setting: keyof Settings, value: string) => string | undefined,
): ReplaceWords {
	const named = replaced.named === 0 ? "none of them named" : `${replaced.named.toLocaleString("en-US")} of them named`;
	const counted = `${count(replaced.communities, "community", "communities")}, ${named}.`;
	const will =
		replaced.communities_will === "change"
			? replaced.changed.length > 0
				? "Its communities will change: they are cut again with settings that differ from when it was made."
				: "Its communities will change: they were cut by an earlier way of clustering, and are cut again by the current one."
			: "Its communities may change: the settings that cut them are the same, but they come out otherwise if its papers changed.";
	const changes = replaced.changed.map(
		({ setting, was, now }) =>
			`${settingLabel(setting)}: ${valueWords(was, (value) => valueLabel?.(setting, value))} → ${valueWords(now, (value) => valueLabel?.(setting, value))}`,
	);
	const applies = "their names, their overviews and whatever was asked about them (tables, stances, answers)";
	const consequence =
		replaced.communities_will === "change"
			? `Once they change, ${applies} no longer apply.`
			: `If they change, ${applies} no longer apply.`;
	return { title: `Make "${replaced.title}" again?`, counted, will, changes, consequence };
}
