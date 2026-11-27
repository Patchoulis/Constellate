"use client";

/**
 * The language model chosen in the settings, for every feature that shows what asking it would cost: an estimate
 * is for one provider and model (its `provider` and `model`), and the choice can change while it is on screen (the
 * sidebar's menu, the settings dialog, another window). Every feature asks for its estimates again when the choice
 * changes (`choiceKey` is in their dependencies), and the button that accepts an estimate is offered only while it
 * is the one for the model chosen now (`estimateFor`): an old figure is never accepted for a new model.
 */
import { createContext, useContext } from "react";
import type { AskingCost, Effort, ProviderId, Settings } from "@rp/contracts";
import type { Async } from "@/lib/use-async";

export interface ChosenModel {
	/** null while the settings name no provider (the engine's default is then asked) */
	provider: ProviderId | null;
	model: string;
	effort: Effort | null;
}

/** The choice as the settings hold it; null until they are read. */
export function chosenModelOf(
	settings: Pick<Settings, "describe_provider" | "describe_model" | "describe_effort"> | undefined,
): ChosenModel | null {
	if (!settings) return null;
	return { provider: settings.describe_provider ?? null, model: settings.describe_model, effort: settings.describe_effort ?? null };
}

/** Provided by the page (app/page.tsx) from the settings it holds. */
export const ChosenModelContext = createContext<ChosenModel | null>(null);

export function useChosenModel(): ChosenModel | null {
	return useContext(ChosenModelContext);
}

/** The choice as one dependency: estimates are asked for again when it changes. */
export function choiceKey(chosen: ChosenModel | null): string {
	return chosen ? `${chosen.provider ?? ""}\n${chosen.model}\n${chosen.effort ?? ""}` : "";
}

/** Is this estimate for the model chosen now? Yes too while the choice is not known yet (the engine's is then all
 *  there is, and the engine refuses a request for another model than the one it would ask). */
export function isFor(estimate: Pick<AskingCost, "provider" | "model">, chosen: ChosenModel | null): boolean {
	if (!chosen) return true;
	return estimate.model === chosen.model && (chosen.provider === null || estimate.provider === chosen.provider);
}

/**
 * The estimate that may be shown beside a button that accepts it: answered for exactly what is asked now, and for
 * the model chosen now. Undefined while it is being asked for (again), after asking failed, or once the choice
 * changed: the old figure is not shown, and nothing can accept it.
 */
export function currentEstimate<T extends Pick<AskingCost, "provider" | "model">>(
	estimate: Async<T>,
	chosen: ChosenModel | null,
): T | undefined {
	return estimate.status === "ready" && isFor(estimate.data, chosen) ? estimate.data : undefined;
}
