/**
 * The choice of language model, as pure logic: providers grouped with their models, which can be used and why
 * not, the efforts a model has, and what a model costs, in words with their currency. The engine tells all of it
 * (`GET /api/models`). Until it has answered (or when asking failed), the model the settings name is all there is
 * to show, and nothing can be picked.
 */
import type {
	CatalogItem,
	Effort,
	ModelCatalog,
	ModelEntry,
	ModelPrice,
	ModelProvider,
	ProviderId,
	ReadingState,
	Settings,
} from "@rp/contracts";
import { money, type Currency } from "@/lib/units";

/**
 * Which of the two choices of language model: the one that names communities and writes their overviews, or the
 * one that reads papers (tables, agree / disagree, questions, claims, weaknesses, explanations, citation kinds).
 * The second is the first until it is chosen on its own.
 */
export type ModelRole = "naming" | "reading";

/** The settings each choice is read from and saved to. */
export const ROLE_FIELDS = {
	naming: { provider: "describe_provider", model: "describe_model", effort: "describe_effort" },
	reading: { provider: "reading_provider", model: "reading_model", effort: "reading_effort" },
} as const satisfies Record<ModelRole, { provider: keyof Settings; model: keyof Settings; effort: keyof Settings }>;

/** What the reading model's menu says of itself, wherever it is shown: it is one setting, shared by every feature. */
export const READING_TITLE =
	"The model that reads papers: tables, agree / disagree, questions, claims, weaknesses, explanations and citation kinds. One setting, shared by all of them; changing it here changes it for every one.";

/** The settings that choose the models, as far as they are known. */
export type ModelSettings = Partial<
	Pick<
		Settings,
		| "describe_provider"
		| "describe_model"
		| "describe_effort"
		| "reading_provider"
		| "reading_model"
		| "reading_effort"
		| "reading_same_as_naming"
	>
>;

/** How the model that reads papers is chosen: by default (an older engine or file says nothing: the default), as
 *  the naming model, or a model of its own. */
export function readingStateOf(settings: ModelSettings | undefined): ReadingState {
	if (settings?.reading_provider !== undefined && settings.reading_provider !== null) return "chosen";
	return settings?.reading_same_as_naming ? "same_as_naming" : "default";
}

/**
 * The model that reads papers by default while `provider` names communities, as the engine's catalogue tells it
 * (the Claude API's Haiku): null when that provider has none (the naming model then reads them), undefined while
 * it is not known (no catalogue, or an older engine's).
 */
export function readingDefaultOf(
	catalog: ModelCatalog | undefined,
	provider: ProviderId | undefined,
): { model: string; effort: Effort } | null | undefined {
	const row = catalog?.providers.find((one) => one.id === (provider ?? catalog.chosen.provider));
	return row?.reading_default;
}

/** The provider, model and effort the settings choose for `role`: the reading model's own, the naming one's, or
 *  the default (which needs `catalog`). Each part is undefined while it is not known (settings not read, no
 *  catalogue, an older engine). */
export function pickedOf(
	settings: ModelSettings | undefined,
	role: ModelRole,
	catalog?: ModelCatalog,
): { provider: ProviderId | undefined; model: string | undefined; effort: Effort | undefined } {
	const naming = { provider: settings?.describe_provider, model: settings?.describe_model, effort: settings?.describe_effort };
	if (role === "naming") return naming;
	switch (readingStateOf(settings)) {
		case "chosen":
			return {
				provider: settings?.reading_provider ?? undefined,
				model: settings?.reading_model ?? naming.model,
				effort: settings?.reading_effort ?? naming.effort,
			};
		case "same_as_naming":
			return naming;
		case "default": {
			const fallback = readingDefaultOf(catalog, naming.provider);
			if (fallback === null) return naming;
			if (fallback === undefined) return { provider: undefined, model: undefined, effort: undefined };
			return { provider: naming.provider ?? catalog?.chosen.provider, model: fallback.model, effort: fallback.effort };
		}
	}
}

/** What to save to set the model that reads papers back to the default. */
export const DEFAULT_READING: Partial<Settings> = { reading_provider: null, reading_same_as_naming: false };

/** What to save to read papers with the naming model, rather than the default. */
export const SAME_AS_NAMING: Partial<Settings> = { reading_same_as_naming: true };

/** The first entry of the reading model's menu: what the default is ("Default: Claude Haiku 5.5 · Anthropic"), or,
 *  where the naming provider has no default of its own, the naming model it then is. */
export function defaultEntryLabel(catalog: ModelCatalog, settings: ModelSettings | undefined): string {
	const naming = chosenOf(catalog, settings, "naming");
	const fallback = readingDefaultOf(catalog, naming.providerId);
	if (!fallback) return "The same as the naming model";
	const model = naming.provider?.models.find((one) => one.id === fallback.model);
	return `Default: ${model?.label ?? fallback.model} · ${naming.provider?.label ?? naming.providerId}`;
}

/** Does the naming provider read papers with a default of its own, which "the same as the naming model" is not? */
export function hasOwnDefault(catalog: ModelCatalog, settings: ModelSettings | undefined): boolean {
	return Boolean(readingDefaultOf(catalog, chosenOf(catalog, settings, "naming").providerId));
}

/** How the model that reads papers is chosen, in words for beside it ("Default: Claude Haiku 5.5 · Anthropic.",
 *  "The same as the naming model."); null for a model chosen, which its menu names. */
export function readingStateWords(catalog: ModelCatalog, settings: ModelSettings | undefined): string | null {
	switch (readingStateOf(settings)) {
		case "chosen":
			return null;
		case "same_as_naming":
			return "The same as the naming model.";
		case "default":
			return `${defaultEntryLabel(catalog, settings)}.`;
	}
}

/** One of the entries above the models in the reading model's menu. */
export interface ReadingEntry {
	label: string;
	/** quieter, on the right: the naming model it is, when it is that */
	note: string;
	ticked: boolean;
	change: Partial<Settings>;
}

/**
 * The entries above the models in the reading model's menu: the default, saying what it is, and, where the
 * naming provider has a default of its own (the Claude API), the naming model as a choice of its own; elsewhere
 * the default is the naming model, and one entry says so.
 */
export function readingEntries(catalog: ModelCatalog, settings: ModelSettings | undefined): ReadingEntry[] {
	const state = readingStateOf(settings);
	const naming = chosenLabel(chosenOf(catalog, settings, "naming"));
	if (!hasOwnDefault(catalog, settings)) {
		return [{ label: "The same as the naming model", note: naming, ticked: state !== "chosen", change: DEFAULT_READING }];
	}
	return [
		{ label: defaultEntryLabel(catalog, settings), note: "", ticked: state === "default", change: DEFAULT_READING },
		{ label: "The same as the naming model", note: naming, ticked: state === "same_as_naming", change: SAME_AS_NAMING },
	];
}

/** What can be chosen: the engine's catalog once it has answered; before that (or when asking failed, `why`), only
 *  the model the settings name. */
export type Choosable = { source: "engine"; catalog: ModelCatalog } | { source: "waiting"; catalog: ModelCatalog; why: string | null };

/** The efforts, lowest first, as the engine ranks them. */
export const EFFORTS: Effort[] = ["low", "medium", "high", "xhigh", "max"];

/** What the interface shows for an effort: a menu entry or a chip standing alone, so it begins with a capital. */
export const EFFORT_LABEL: Record<Effort, string> = {
	low: "Low effort",
	medium: "Medium effort",
	high: "High effort",
	xhigh: "Extra-high effort",
	max: "Most effort",
};

/** The model the settings name, alone, while the engine's catalog is not known: shown, not offered. */
export function waitingFor(
	models: CatalogItem[],
	settings: Partial<Pick<Settings, "describe_provider" | "describe_model">> | undefined,
	why: string | null,
): Choosable {
	const id = settings?.describe_provider ?? "anthropic";
	const model = settings?.describe_model ?? models[0]?.name ?? "";
	const known = models.find((item) => item.name === model);
	const only: ModelProvider = {
		id,
		label: id,
		key: { variable: "", needed: false, set: true },
		address: { variable: "", host: "", place: "provider" },
		available: why ? { state: "unavailable", why } : { state: "ready" },
		ids: "listed",
		capabilities: { answers: "enforced", effort: "not_sent", caching: "marked", counting: "endpoint" },
		limits: [],
		models: [{ id: model, label: known?.label ?? model, note: known?.note ?? "", efforts: [], price: { kind: "not_known" } }],
		price_of_typed: { kind: "not_known" },
		docs: "",
	};
	const chosen = { provider: id, model, effort: "medium" as const };
	return { source: "waiting", why, catalog: { providers: [only], chosen, reading: { ...chosen, state: "default" as const } } };
}

/** What can be chosen: the engine's catalog when it has told it, else the settings' model alone. */
export function choosableOf(
	engine: ModelCatalog | undefined,
	models: CatalogItem[],
	settings: Partial<Pick<Settings, "describe_provider" | "describe_model">> | undefined,
	why: string | null = null,
): Choosable {
	return engine ? { source: "engine", catalog: engine } : waitingFor(models, settings, why);
}

/** A label as it stands alone in the interface: its first letter a capital, the rest as given ("Set OPENAI_API_KEY"). */
export function capitalised(text: string): string {
	return text.charAt(0).toUpperCase() + text.slice(1);
}

/** Why a provider cannot be used ("Set OPENAI_API_KEY"), or null when it can. The engine says it in lower case, as
 *  the end of a sentence; here it stands alone. */
export function unavailableBecause(provider: ModelProvider): string | null {
	if (provider.available.state === "ready") return null;
	// a state this version does not know (a newer engine) may come without its reason
	const why = (provider.available as { why?: unknown }).why;
	return typeof why === "string" && why ? capitalised(why) : "Not available";
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

/** The model chosen for `role`, as the settings name it, else as the engine's catalog does. */
export function chosenOf(catalog: ModelCatalog, settings: ModelSettings | undefined, role: ModelRole = "naming"): Chosen {
	const picked = pickedOf(settings, role, catalog);
	// the catalog's own, for what the settings do not say: an older engine tells no reading model, which is the naming one
	const told = role === "reading" ? (catalog.reading ?? catalog.chosen) : catalog.chosen;
	const providerId = picked.provider ?? told.provider;
	const id = picked.model ?? told.model;
	const provider = catalog.providers.find((one) => one.id === providerId);
	return {
		providerId,
		provider,
		id,
		model: provider?.models.find((one) => one.id === id),
		effort: picked.effort ?? told.effort,
	};
}

/** Are two choices the same provider, model and effort? */
export function sameChoice(one: Chosen, other: Chosen): boolean {
	return one.providerId === other.providerId && one.id === other.id && one.effort === other.effort;
}

/** Does another model read papers than the one that names communities? */
export function readingDiffers(catalog: ModelCatalog, settings: ModelSettings | undefined): boolean {
	return !sameChoice(chosenOf(catalog, settings, "naming"), chosenOf(catalog, settings, "reading"));
}

/** Are the two models shown apart, each for what it does? When another reads papers (the default included), and also
 *  when the same model was chosen to read on its own: changing the naming one would then not change the other. */
export function modelsApart(catalog: ModelCatalog, settings: ModelSettings | undefined): boolean {
	return readingStateOf(settings) === "chosen" || readingDiffers(catalog, settings);
}

/**
 * What a window may do with the model that reads papers when the server owns the providers (`providerOwned`):
 * choose none while none is chosen (a pick would pin it to the naming provider of now: the owner chooses it), and
 * move between the default and the naming model (one provider, the server's) only while what it reads with now is
 * of that provider.
 */
export function readingPicks(
	catalog: ModelCatalog,
	settings: ModelSettings | undefined,
	providerOwned: boolean,
): { locked: boolean; canReset: boolean } {
	if (!providerOwned) return { locked: false, canReset: true };
	const free = readingStateOf(settings) !== "chosen";
	const naming = chosenOf(catalog, settings, "naming");
	const reading = chosenOf(catalog, settings, "reading");
	return { locked: free, canReset: free || naming.providerId === reading.providerId };
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

/** What to save when a model is picked for `role`. The provider is left out where the window may not change it (a
 *  server's): the engine then takes the model as one of the provider `role` asks now. */
export function changeFor(providerId: ProviderId, id: string, providerOwned: boolean, role: ModelRole = "naming"): Partial<Settings> {
	const fields = ROLE_FIELDS[role];
	return providerOwned ? { [fields.model]: id } : { [fields.provider]: providerId, [fields.model]: id };
}

/** What to save when an effort is picked for `role`. */
export function effortChange(effort: Effort, role: ModelRole = "naming"): Partial<Settings> {
	return { [ROLE_FIELDS[role].effort]: effort };
}
