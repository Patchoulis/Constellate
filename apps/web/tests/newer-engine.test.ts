/**
 * What an engine of another version can send: a state, kind or tier this interface does not know. The app updated
 * under an open window, or a window open on a server of another version, must show something and never throw.
 */
import assert from "node:assert/strict";
import { test } from "node:test";
import type { CitationLink, Evidence, ModelProvider, Status } from "@rp/contracts";
import { tagText, tagTitle } from "@/lib/acceptance";
import { judgementLabel } from "@/lib/features/citation_contexts/citation-kinds";
import { linkTag, lookupsStoppedLine } from "@/lib/reader/citations";
import { evidenceTitle } from "@/lib/reader/evidence";
import { unavailableBecause } from "@/lib/shell/models";
import { listedOfRow } from "@/lib/shell/runs-list";
import { started, step } from "@/lib/shell/use-followed-job";

const unknown = <T>(value: unknown) => value as T;

test("a job in a state this version does not know is still under way, not undefined", () => {
	const job = started(4);
	const next = step(job as Extract<typeof job, { state: "running" }>, unknown<Status>({ id: 4, state: "queued", log: ["waiting"], result: null, error: null, total: 1 }));
	assert.equal(next.state, "running");
	assert.deepEqual(next.log, ["waiting"]);
});

test("a run listed in a kind this version does not know is listed by its name", () => {
	const listed = listedOfRow(unknown({ kind: "importing", name: "neurips-2025" }));
	assert.equal(listed.name, "neurips-2025");
	assert.equal(listed.problem, null);
});

test("a citation's reading of a kind this version does not know is said as not classified", () => {
	assert.equal(judgementLabel(unknown({ state: "disputed" })).label, "not classified");
	assert.equal(judgementLabel(unknown({ state: "justified", kind: "rebuttal" })).label, "rebuttal");
});

test("a quote found in a way this version does not know still says where it is from", () => {
	const evidence = (established: unknown): Evidence => ({ paper: "p", quote: "words", established: unknown(established) });
	assert.match(evidenceTitle(evidence({ kind: "abstract", field: "abstract", start: 0, end: 5, matched: { kind: "normalised" } }), "P"), /Found in the paper/);
	assert.match(evidenceTitle(evidence({ kind: "abstract", field: "title", start: 0, end: 5, matched: { kind: "close" } }), "P"), /Found in the paper/);
	assert.match(evidenceTitle(evidence({ kind: "in_figure" }), "P"), /^“words”\nP\. Click to open the paper\.$/);
});

test("a reference found to be something this version does not know is tagged, and a stop without a reason says none", () => {
	assert.equal(linkTag(unknown<CitationLink>({ kind: "doi", doi: "10.1/x" })).label, "Elsewhere");
	assert.match(lookupsStoppedLine(unknown({ kind: "rate_limited", left: 3 })) ?? "", /^The lookups of the works this paper cites stopped early\. 3 cited works/);
});

test("a provider unavailable for a reason not given says it is not available", () => {
	assert.equal(unavailableBecause(unknown<ModelProvider>({ available: { state: "paused" } })), "Not available");
	assert.equal(unavailableBecause(unknown<ModelProvider>({ available: { state: "unavailable", why: "set OPENAI_API_KEY" } })), "Set OPENAI_API_KEY");
});

test("an acceptance tier this version does not know is named as the engine names it", () => {
	assert.equal(tagText(unknown("award")), "Award");
	assert.equal(tagTitle(unknown("award")), "Accepted as award.");
	assert.equal(tagText("oral"), "Oral");
});
