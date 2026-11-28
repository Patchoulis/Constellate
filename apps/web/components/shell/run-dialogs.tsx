"use client";

import { useCallback } from "react";
import type { Catalog, CatalogItem, DescribeEstimate, Settings } from "@rp/contracts";
import { SETTING_LABELS } from "@/components/shell/settings-dialog";
import { useChoose } from "@/components/shell/confirm";
import { costTitle, costWords } from "@/lib/cost";
import { type NamingMode, namingWords } from "@/lib/features/writing/naming";
import type { Answer, Asking, NamingOffer } from "@/lib/shell/run-replace";
import { acceptedWords, replaceWords } from "@/lib/shell/run-replace";

/** A setting by the name the settings dialog gives it. */
const settingLabel = (setting: keyof Settings) => SETTING_LABELS[setting] ?? setting.replaceAll("_", " ");

/** A value by the name its menu shows, when it has one ("Spectral" for "spectral"). */
function valueLabelOf(catalog: Catalog | undefined) {
	return (setting: keyof Settings, value: string): string | undefined => {
		const from: CatalogItem[] | undefined =
			setting === "clustering"
				? catalog?.clustering
				: setting === "embedding_model"
					? catalog?.embedding_models
					: setting === "acceptance_filter"
						? catalog?.acceptance
						: setting === "exclude_flags"
							? catalog?.flags.flatMap((group) => group.items)
							: undefined;
		return from?.find((item) => item.name === value)?.label;
	};
}

function NamingPart({ naming }: { naming: NamingOffer }) {
	if (naming.kind === "unavailable") {
		return <p>No key is set for the chosen language model, so the communities are not named again: their names are kept as they are.</p>;
	}
	const { plan } = naming;
	return (
		<p title={costTitle(plan.estimate)}>
			Naming the communities again asks {plan.model}: {costWords(plan.estimate)} for up to{" "}
			{plan.communities_at_most.toLocaleString("en-US")} communities (an estimate). Without it, names kept from before for the same papers
			are given again and the others stay unnamed.
		</p>
	);
}

/**
 * Ask before a run request that would replace a run or pay for names (lib/shell/run-replace.ts `startRun`): what would be
 * replaced, in the settings dialog's words, and what naming would cost. Cancel is the default.
 */
export function useAskRun(catalog: Catalog | undefined): (asking: Asking) => Promise<Answer> {
	const choose = useChoose();
	return useCallback(
		async ({ replaced, naming, accepted }: Asking) => {
			const words = replaced ? replaceWords(replaced, settingLabel, valueLabelOf(catalog)) : null;
			const go = replaced ? "Make it again" : "Run";
			const choices: { value: "named" | "unnamed"; label: string; title?: string }[] =
				naming?.kind === "offer"
					? [
							{ value: "unnamed", label: `${go} without naming` },
							{ value: "named", label: `${go} and name them`, title: costTitle(naming.plan.estimate) },
						]
					: [{ value: "unnamed", label: go }];
			return choose({
				title: words?.title ?? "Name the communities?",
				destructive: replaced !== null,
				choices,
				body: (
					<>
						{words && (
							<>
								<p className="text-foreground">{words.counted}</p>
								<p>{words.will}</p>
								{words.changes.length > 0 && (
									<ul className="list-disc pl-4">
										{words.changes.map((change) => (
											<li key={change}>{change}</li>
										))}
									</ul>
								)}
								<p>{words.consequence}</p>
							</>
						)}
						{naming && <NamingPart naming={naming} />}
						{accepted && <p>{acceptedWords(accepted)}</p>}
					</>
				),
			});
		},
		[choose, catalog],
	);
}

/** The dialog's title and its paid button, for what is asked. */
const NAMING_ASKED: Record<NamingMode, { title: (run: string) => string; go: string }> = {
	name: { title: (run) => `Name the communities of "${run}"?`, go: "Name them" },
	sections: { title: (run) => `Rewrite the plain descriptions of "${run}" with sections and citations?`, go: "Rewrite them" },
	again: { title: (run) => `Name every community of "${run}" again?`, go: "Name them again" },
};

/**
 * Ask before naming a made run's communities (or rewriting their plain descriptions with sections, or naming all
 * again): how many there are, how many are named or plain, and the cost.
 */
export function useAskNaming(): (estimate: DescribeEstimate, mode: NamingMode, title: string) => Promise<boolean> {
	const choose = useChoose();
	return useCallback(
		async (estimate: DescribeEstimate, mode: NamingMode, title: string) => {
			const words = namingWords(estimate, undefined, undefined, mode);
			if (!estimate.api_key_set) {
				await choose({
					title: `The communities of "${title}" cannot be ${mode === "sections" ? "rewritten" : "named"} now`,
					cancel: "Close",
					choices: [],
					body: <p>No key is set for {estimate.model}&apos;s provider. Settings, Language model, says which variable to set.</p>,
				});
				return false;
			}
			const asked = NAMING_ASKED[mode];
			const answer = await choose({
				title: asked.title(title),
				cancel: words.nothing ? "Close" : "Cancel",
				choices: words.nothing ? [] : [{ value: "yes", label: asked.go, title: costTitle(estimate.estimate) }],
				body: (
					<>
						<p className="text-foreground">{words.counted}</p>
						<p>{words.asked}</p>
						{words.cost && (
							<p title={costTitle(estimate.estimate)}>
								It costs {words.cost} (an estimate): no call is started that could pass the most shown.
							</p>
						)}
						{mode === "again" && !words.nothing && <p>The names and descriptions they have now are replaced by the new ones.</p>}
						{mode === "sections" && !words.nothing && (
							<p>
								Each plain description is replaced by three sections (Problem, Shared methods, What sets it apart), each citing the papers
								it rests on. A section is shown only when its quotes are found in at least 2 of the community&apos;s papers.
							</p>
						)}
					</>
				),
			});
			return answer === "yes";
		},
		[choose],
	);
}
