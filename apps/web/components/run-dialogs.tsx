"use client";

import { useCallback } from "react";
import type { Catalog, CatalogItem, DescribeEstimate, Settings } from "@rp/contracts";
import { SETTING_LABELS } from "@/components/settings-dialog";
import { useChoose } from "@/components/ui/confirm";
import { costTitle, costWords } from "@/lib/cost";
import { namingWords } from "@/lib/features/writing/naming";
import type { Answer, Asking, NamingOffer } from "@/lib/run-replace";
import { acceptedWords, replaceWords } from "@/lib/run-replace";

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
 * Ask before a run request that would replace a run or pay for names (lib/run-replace.ts `startRun`): what would be
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

/** Ask before naming a made run's communities: how many there are, how many are named, and the cost. */
export function useAskNaming(): (estimate: DescribeEstimate, redo: boolean, title: string) => Promise<boolean> {
	const choose = useChoose();
	return useCallback(
		async (estimate: DescribeEstimate, redo: boolean, title: string) => {
			const words = namingWords(estimate);
			if (!estimate.api_key_set) {
				await choose({
					title: `The communities of "${title}" cannot be named now`,
					cancel: "Close",
					choices: [],
					body: <p>No key is set for {estimate.model}&apos;s provider. Settings, Language model, says which variable to set.</p>,
				});
				return false;
			}
			const answer = await choose({
				title: redo ? `Name every community of "${title}" again?` : `Name the communities of "${title}"?`,
				cancel: words.nothing ? "Close" : "Cancel",
				choices: words.nothing
					? []
					: [{ value: "yes", label: redo ? "Name them again" : "Name them", title: costTitle(estimate.estimate) }],
				body: (
					<>
						<p className="text-foreground">{words.counted}</p>
						<p>{words.asked}</p>
						{words.cost && (
							<p title={costTitle(estimate.estimate)}>
								It costs {words.cost} (an estimate): no call is started that could pass the most shown.
							</p>
						)}
						{redo && !words.nothing && <p>The names and descriptions they have now are replaced by the new ones.</p>}
					</>
				),
			});
			return answer === "yes";
		},
		[choose],
	);
}
