/** A community's uniqueness: how far its papers are from what it is about, and how that is said. */
import assert from "node:assert/strict";
import { test } from "node:test";
import { UNIQUENESS_NOTE, uniqueness } from "@/lib/community/community";
import { distance } from "@/lib/units";

test("uniqueness is 1 minus the papers' mean cosine similarity to the community's mean embedding", () => {
	assert.equal(uniqueness({ similarity_to_mean: 1 }), 0, "every paper the same: no uniqueness at all");
	assert.ok(Math.abs(uniqueness({ similarity_to_mean: 0.707 }) - 0.293) < 1e-9);
	assert.equal(uniqueness({ similarity_to_mean: 0 }), 1);
});

test("papers doing the same thing give a lower uniqueness than varied ones, and it is never below 0", () => {
	const same = uniqueness({ similarity_to_mean: 0.92 });
	const varied = uniqueness({ similarity_to_mean: 0.55 });
	assert.ok(same < varied);
	// a similarity a hair above 1 is rounding in the sums, not a negative distance
	assert.equal(uniqueness({ similarity_to_mean: 1.0000001 }), 0);
});

test("a uniqueness is shown with its unit, and the note says which way is which", () => {
	assert.equal(distance(0.2931), "0.29 cosine distance");
	assert.equal(distance(0.2931, 3), "0.293 cosine distance");
	assert.match(UNIQUENESS_NOTE, /Low: its papers are all doing much the same thing/);
	assert.match(UNIQUENESS_NOTE, /High: the work in it is more varied/);
});
