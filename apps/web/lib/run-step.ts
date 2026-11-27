/**
 * The step of the run being made, in one line for the job view: "Downloading the embedding model: 312 of 1,180 MB
 * (step 2 of 10)", "Embed: 3,120 of 12,487 papers (step 3 of 10)". Bytes are shown in megabytes of 1,048,576
 * bytes, rounded, as the engine's own log shows them (pipeline/steps.rs `Count`).
 */
import type { RunStep, StepCount } from "@rp/contracts";

const thousands = (value: number) => Math.round(value).toLocaleString("en-US");

/** "312 of 1,180 MB", "3,120 of 12,487 papers". */
export function countWords(count: StepCount): string {
	if (count.unit === "bytes") return `${thousands(count.done / 1_048_576)} of ${thousands(count.of / 1_048_576)} MB`;
	return `${thousands(count.done)} of ${thousands(count.of)} ${count.unit}`;
}

/** The whole line. A step that downloads is said as a download; the others by their title, capitalised. */
export function stepLine(step: RunStep): string {
	const what = step.count?.unit === "bytes" ? `downloading ${step.title}` : step.title;
	const counted = step.count ? `: ${countWords(step.count)}` : "";
	const detail = step.detail ? ` · ${step.detail}` : "";
	return `${what.charAt(0).toUpperCase()}${what.slice(1)}${counted} (step ${step.number} of ${step.of})${detail}`;
}
