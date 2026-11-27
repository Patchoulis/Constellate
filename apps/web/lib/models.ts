/**
 * The choice of language model, as pure logic: providers grouped with their models, which can be used and why
 * not, the efforts a model has, and what a model costs, in words with their currency. The engine tells all of it
 * (`GET /api/models`); the Python backend, which knows the Claude API only, does not, and then today's list of
 * Claude models is all there is to choose from.
 */
import type { CatalogItem, Effort, ModelCatalog, ModelEntry, ModelPrice, ModelProvider, ProviderId, Settings } from "@rp/contracts";
import { isMissingRoute } from "@/lib/missing-route";
import { money, type Currency } from "@/lib/units";

/** Where the choice came from: the engine, which knows every provider, or an older backend, which knows Claude only. */
export type Choosable = { source: "engine"; catalog: ModelCatalog } | { source: "older_backend"; catalog: ModelCatalog };

/** The efforts, lowest first, as the engine ranks them. */
export const EFFORTS: Effort[] = ["low", "medium", "high", "xhigh", "max"];

/** What the interface shows for an effort. */
export const EFFORT_LABEL: Record<Effort, string> = {
	low: "low effort",
	medium: "medium effort",
	high: "high effort",
	xhigh: "extra-high effort",
	max: "most effort",
};

/** The Claude API alone, with the models a backend without `GET /api/models` lists: what the app offered before. */
export function olderBackend(models: CatalogItem[], chosen: string | undefined): Choosable {
	const anthropic: ModelProvider = {
		id: "anthropic",
		label: "Anthropic",
		key: { variable: "ANTHROPIC_API_KEY", needed: true, set: true },
		address: { variable: "ANTHROPIC_BASE_URL", host: "api.anthropic.com", place: "provider" },
		available: { state: "ready" },
		ids: "listed",
		capabilities: { answers: "enforced", effort: "not_sent", caching: "marked", counting: "endpoint" },
		limits: [],
		models: models.map((item) => ({
			id: item.name,
			label: item.label ?? item.name,
			note: item.note,
			efforts: [],
			price: { kind: "not_known" },
		})),
		price_of_typed: { kind: "not_known" },
		docs: "",
	};
	const model = chosen ?? models[0]?.name ?? "";
	return { source: "older_backend", catalog: { providers: [anthropic], chosen: { provider: "anthropic", model, effort: "medium" } } };
}

/** The engine's catalog, or null from a backend that has no `GET /api/models`. */
export async function engineCatalog(fetchCatalog: () => Promise<ModelCatalog>): Promise<ModelCatalog | null> {
	try {
		return await fetchCatalog();
	} catch (error) {
		if (isMissingRoute(error)) return null;
		throw error;
	}
}

/** What can be chosen: the engine's catalog when it told one, else today's list of Claude models. */
export function choosableOf(engine: ModelCatalog | null | undefined, models: CatalogItem[], chosen: string | undefined): Choosable {
	return engine ? { source: "engine", catalog: engine } : olderBackend(models, chosen);
}

/** Why a provider cannot be used ("set OPENAI_API_KEY"), or null when it can. */
export function unavailableBecause(provider: ModelProvider): string | null {
	return provider.available.state === "ready" ? null : provider.available.why;
}

/** One provider's part of the menu. */
export interface ModelGroup {
	provider: ModelProvider;
	/** null when it can be used */
	why: string | null;
	models: ModelEntry[];
	/** may an id it does not list be typed? */
	typed: boolean;
}

/** The menu: providers that can be used first, then those that cannot (with why), each in the engine's order. */
export function groupsOf(catalog: ModelCatalog): ModelGroup[] {
	const groups = catalog.providers.map((provider) => ({
		provider,
		why: unavailableBecause(provider),
		models: provider.models,
		typed: provider.ids !== "listed",
	}));
	return [...groups.filter((group) => group.why === null), ...groups.filter((group) => group.why !== null)];
}

/** A model id as providers write them: what the engine takes for a typed id. */
export function isPlainId(id: string): boolean {
	return id.length > 0 && id.length <= 200 && !/^[./]/.test(id) && !id.includes("..") && /^[A-Za-z0-9._/:-]+$/.test(id);
}

/**
 * The model ids an OpenAI-compatible server says it has (`GET /api/models/endpoint`), as far as they can be used:
 * ids that are not plain (not text, too long, a path) are dropped and counted. Null when the answer is not a list
 * of models at all (a server that answers something else on that address).
 */
export function endpointIds(answer: unknown): { ids: string[]; dropped: number } | null {
	const models = (answer as { models?: unknown } | null)?.models;
	if (!Array.isArray(models)) return null;
	const ids = [...new Set(models.filter((id): id is string => typeof id === "string" && isPlainId(id)))];
	return { ids, dropped: models.length - ids.length };
}

/** The chosen provider and model as the settings name them, with the provider's row and the model's when listed. */
export interface Chosen {
	providerId: ProviderId;
	provider: ModelProvider | undefined;
	id: string;
	/** undefined for an id typed rather than listed */
	model: ModelEntry | undefined;
	effort: Effort;
}

export function chosenOf(
	catalog: ModelCatalog,
	settings: Pick<Settings, "describe_provider" | "describe_model" | "describe_effort"> | undefined,
): Chosen {
	const providerId = settings?.describe_provider ?? catalog.chosen.provider;
	const id = settings?.describe_model ?? catalog.chosen.model;
	const provider = catalog.providers.find((one) => one.id === providerId);
	return {
		providerId,
		provider,
		id,
		model: provider?.models.find((one) => one.id === id),
		effort: settings?.describe_effort ?? catalog.chosen.effort,
	};
}

/** "Claude Opus 5.5 · Anthropic", "qwen3:32b · OpenAI-compatible endpoint". */
export function chosenLabel(chosen: Chosen): string {
	return `${chosen.model?.label ?? chosen.id} · ${chosen.provider?.label ?? chosen.providerId}`;
}

/** The price of model `id` at `provider`: the listed one's, or what a typed id has (not known, or no fee here). */
export function priceOf(provider: ModelProvider | undefined, id: string): ModelPrice {
	if (!provider) return { kind: "not_known" };
	return provider.models.find((one) => one.id === id)?.price ?? provider.price_of_typed;
}

/**
 * A price in words, every number with its currency: "USD 2.00 in, USD 10.00 out per million tokens"; null (nothing
 * is shown: no "not known", no 0) when it is not; "runs on this computer: no API cost" for a server here.
 */
export function priceLabel(price: ModelPrice, currency: Currency, locale?: string): string | null {
	switch (price.kind) {
		case "known":
			return `${money(price.input, currency, locale)} in, ${money(price.output, currency, locale)} out per million tokens`;
		case "not_known":
			return null; // no price: nothing is shown, never "not known" or 0
		case "this_computer":
			return "runs on this computer: no API cost";
	}
}

/** The efforts the chosen model takes: those listed with it, none for a typed id or a provider that sets none. */
export function effortsOf(chosen: Chosen): Effort[] {
	if (chosen.provider?.capabilities.effort === "not_sent") return [];
	return chosen.model?.efforts ?? [];
}

/** The effort the engine will send for `asked` to a model that has `efforts`: the nearest, the lower of two as near. */
export function effortSent(asked: Effort, efforts: Effort[]): Effort | null {
	const rank = (effort: Effort) => EFFORTS.indexOf(effort);
	let best: Effort | null = null;
	for (const effort of efforts) {
		if (best === null) best = effort;
		const [distance, bestDistance] = [Math.abs(rank(effort) - rank(asked)), Math.abs(rank(best) - rank(asked))];
		if (distance < bestDistance || (distance === bestDistance && rank(effort) < rank(best))) best = effort;
	}
	return best;
}

/** What to save when a model is picked. The provider is left out where the window may not change it (a server's). */
export function changeFor(providerId: ProviderId, id: string, providerOwned: boolean): Partial<Settings> {
	return providerOwned ? { describe_model: id } : { describe_provider: providerId, describe_model: id };
}
