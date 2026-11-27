/**
 * The step of the run being made, in one line for the job view: "Step 3 of 10: embedding 3,120 of 12,487 papers",
 * "Step 2 of 10: downloading the embedding model: 312 of 1,180 MB". Bytes are shown in megabytes of 1,048,576
 * bytes, rounded, as the engine's own log shows them (pipeline/steps.rs `Count`).
 */
import type { RunStep, RunStepName, StepCount } from "@rp/contracts";

const thousands = (value: number) => Math.round(value).toLocaleString("en-US");

/** What one of each unit is called; slices are the days of arXiv or the conferences a selection is read in. */
const UNIT: Record<StepCount["unit"], { one: string; many: string }> = {
	papers: { one: "paper", many: "papers" },
	communities: { one: "community", many: "communities" },
	slices: { one: "day or conference", many: "days or conferences" },
	bytes: { one: "MB", many: "MB" },
};

/** "312 of 1,180 MB", "3,120 of 12,487 papers", "1 of 4 days or conferences". */
export function countWords(count: StepCount): string {
	if (count.unit === "bytes") return `${thousands(count.done / 1_048_576)} of ${thousands(count.of / 1_048_576)} MB`;
	const unit = UNIT[count.unit] ?? { one: count.unit, many: count.unit };
	return `${thousands(count.done)} of ${thousands(count.of)} ${count.of === 1 ? unit.one : unit.many}`;
}

/** What each step is doing, as a person says it; `counts` is the unit it counts when the count reads as its object
 *  ("embedding 3 of 9 papers"), else the count follows a colon ("finding communities: …"). */
const DOING: Record<RunStepName, { doing: string; counts?: StepCount["unit"] }> = {
	gather: { doing: "gathering the papers" },
	model: { doing: "loading the embedding model" },
	embed: { doing: "embedding", counts: "papers" },
	detect: { doing: "checking abstracts for AI-style wording" },
	filter: { doing: "choosing which papers go in" },
	similarity: { doing: "linking similar papers" },
	communities: { doing: "finding the communities" },
	summarise: { doing: "summarising", counts: "communities" },
	describe: { doing: "naming", counts: "communities" },
	write: { doing: "writing the results" },
};

/** The step in words, with its count when it has one: what follows "Step 3 of 10: ". */
export function doingWords(step: RunStep): string {
	const known = DOING[step.step];
	const { count } = step;
	// the model is downloaded the first time, and loaded after that
	if (step.step === "model" && count?.unit === "bytes") return `downloading the embedding model: ${countWords(count)}`;
	const doing = known?.doing ?? step.title;
	if (!count) return known?.counts ? (step.step === "embed" ? "embedding the papers" : `${doing} the communities`) : doing;
	if (known?.counts === count.unit) return `${doing} ${countWords(count)}`;
	return `${doing}: ${countWords(count)}`;
}

/** The whole line: "Step 3 of 10: embedding 3,120 of 12,487 papers · on the graphics card". */
export function stepLine(step: RunStep): string {
	const detail = step.detail ? ` · ${step.detail}` : "";
	return `Step ${step.number} of ${step.of}: ${doingWords(step)}${detail}`;
}
