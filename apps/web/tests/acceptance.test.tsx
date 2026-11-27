/** A paper's acceptance tag: the conference's own term, drawn as Constellate's tier, and what the chips include. */
import assert from "node:assert/strict";
import { test } from "node:test";
import { renderToStaticMarkup } from "react-dom/server";
import type { AcceptanceTerm, GraphNode } from "@rp/contracts";
import { AcceptanceTag } from "@/components/brand";
import { tagText, tagTitle, termsOf } from "@/lib/acceptance";
import { kindNotes } from "@/lib/paper-filter";

// as the backend sends them (rp/tiers.py), for decisions seen in the conferences' lists
const HIGHLIGHT: AcceptanceTerm = {
	conference: "CVPR 2024",
	called: "Highlight",
	means: "11.9% of the papers in its list; a poster singled out as a highlight",
	known: true,
};
const NOTABLE: AcceptanceTerm = {
	conference: "ICLR 2023",
	called: "Notable top 5%",
	means: "5.7% of the papers in its list; the conference's name that year for its orals",
	known: true,
};
const UNKNOWN: AcceptanceTerm = {
	conference: "CVPR 2027",
	called: "Award Candidate",
	means: "not yet in Constellate's table of this conference's terms, so its tier was read from its wording",
	known: false,
};

const unescaped = (html: string) => html.replace(/&#x27;/g, "'").replace(/&amp;/g, "&");

test("the tag shows the conference's term, coloured by Constellate's tier", () => {
	const highlight = unescaped(renderToStaticMarkup(<AcceptanceTag state="spotlight" term={HIGHLIGHT} />));
	assert.match(highlight, />Highlight<\/span>$/);
	assert.match(
		highlight,
		/title="CVPR 2024's Highlight: 11.9% of the papers in its list; a poster singled out as a highlight. Constellate counts it as a spotlight."/,
	);
	assert.doesNotMatch(highlight, /text-success/);
	const notable = unescaped(renderToStaticMarkup(<AcceptanceTag state="oral" term={NOTABLE} small />));
	assert.match(notable, />Notable top 5%<\/span>$/);
	assert.match(notable, /text-success/);
	assert.match(notable, /counts it as an oral\./);
});

test("a term the table does not know says its tier was read from its wording", () => {
	assert.equal(tagText("oral", UNKNOWN), "Award Candidate");
	assert.equal(
		tagTitle("oral", UNKNOWN),
		"CVPR 2027's Award Candidate: not yet in Constellate's table of this conference's terms, so its tier was read from its wording. Constellate counts it as an oral.",
	);
});

test("a run saved before terms were kept shows the tier's generic name", () => {
	for (const term of [undefined, null]) {
		assert.equal(tagText("oral", term), "Oral");
		assert.equal(tagText("spotlight", term), "Spotlight");
		assert.match(tagTitle("oral", term), /^Accepted as an oral: /);
	}
	assert.match(renderToStaticMarkup(<AcceptanceTag state="spotlight" />), />Spotlight<\/span>$/);
});

test("an ordinary acceptance or a paper not from a conference gets no tag", () => {
	assert.equal(renderToStaticMarkup(<AcceptanceTag state="poster" />), "");
	assert.equal(renderToStaticMarkup(<AcceptanceTag state={null} />), "");
});

test("the chips' hover text lists the conferences' terms they include", () => {
	const node = (id: string, acceptance: GraphNode["acceptance"], term?: AcceptanceTerm): GraphNode => ({
		id,
		title: "",
		community: 0,
		x: 0,
		y: 0,
		ai: false,
		acceptance,
		...(term ? { acceptance_term: term } : {}),
	});
	const nodes = [
		node("a", "spotlight", HIGHLIGHT),
		node("b", "spotlight", HIGHLIGHT),
		node("c", "spotlight", { ...HIGHLIGHT, conference: "ICLR 2023", called: "Notable top 25%" }),
		node("d", "oral", NOTABLE),
		node("e", "poster"),
	];
	assert.deepEqual(termsOf(nodes, "spotlight"), ["Highlight", "Notable top 25%"]);
	const notes = kindNotes(nodes);
	assert.equal(notes.spotlight, "Papers the conference singled out, a tier below an oral. Spotlights here: Highlight, Notable top 25%");
	assert.equal(notes.oral, "Papers the conference picked for a talk. Orals here: Notable top 5%");
	assert.equal(notes.poster, "Ordinary acceptances");
	// a run saved before terms were kept: the generic notes
	assert.equal(kindNotes([node("f", "oral")]).oral, "Papers the conference picked for a talk");
});
