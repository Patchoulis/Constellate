/**
 * Every number the interface shows says what it is a number of: "0.45 cosine similarity", "523 papers",
 * "CAD 1.42", "12k tokens". A bare figure means nothing to someone who did not write the code.
 */

/** "523 papers", "1 paper", "12,487 papers". */
export function count(amount: number, one: string, many = `${one}s`): string {
	return `${amount.toLocaleString()} ${amount === 1 ? one : many}`;
}

/** Cosine similarity of two embeddings: 1 is the same direction, 0 unrelated. */
export function similarity(value: number, digits = 2): string {
	return `${value.toFixed(digits)} cosine similarity`;
}

/** A cosine distance (1 minus a cosine similarity), said with what it is: "0.29 cosine distance". */
export function distance(value: number, digits = 2): string {
	return `${value.toFixed(digits)} cosine distance`;
}

export const SIMILARITY_NOTE =
	"Cosine similarity of the embeddings: 1 means the same direction (as alike as two texts can be), 0 means unrelated.";

/** "12%" of something named by the caller. */
export function percent(share: number): string {
	return `${Math.round(100 * share)}%`;
}

/** A score's scale: its lowest and highest possible score. Reviewers' scales differ by venue and year. */
export interface ScoreScale {
	minimum: number;
	maximum: number;
}

const score = (value: number) => (Number.isInteger(value) ? String(value) : value.toFixed(1));

/** "the 1 to 10 scale". */
export function scaleName(scale: ScoreScale): string {
	return `the ${scale.minimum} to ${scale.maximum} scale`;
}

/** A figure on its venue's scale, the scale named: "5.5 of 10 mean rating" (0-based scales say so: "4 of 0 to 10"). */
export function onScale(value: number, scale: ScoreScale, what: string): string {
	const top = scale.minimum === 1 ? String(scale.maximum) : `${scale.minimum} to ${scale.maximum}`;
	return `${score(value)} of ${top} ${what}`;
}

/** The most stars a scale is drawn with: a longer scale (1 to 100) is said in words only. */
export const MOST_STARS = 10;

/**
 * A score as stars out of its scale's own number of points: 4 on a 1 to 5 scale is 4 of 5 stars, 5.5 on a 1 to 10
 * scale 5.5 of 10. A scale that starts at 0 or below counts from its lowest point (0 to 10 has 10 stars, and 4 is 4 of
 * them). `filled` is kept within the stars there are. Null when the scale has no points or more than MOST_STARS.
 */
export function starsOf(value: number, scale: ScoreScale): { total: number; filled: number } | null {
	const from = scale.minimum >= 1 ? 0 : scale.minimum;
	const total = scale.maximum - from;
	if (!Number.isFinite(total) || !Number.isInteger(total) || total < 1 || total > MOST_STARS) return null;
	return { total, filled: Math.min(total, Math.max(0, value - from)) };
}

/** How far apart reviewers' scores are, in points of the scale: "1.2 points spread". */
export function spreadPoints(spread: number): string {
	return `${spread.toFixed(1)} points spread`;
}

/** A position within a scale (0 at its lowest, 1 at its highest), said to be normalised: "56% of the scale, normalised". */
export function normalised(position: number): string {
	return `${percent(position)} of the scale, normalised`;
}

/** "38k tokens", "1.2M tokens". */
export function tokens(amount: number): string {
	const short = amount >= 1e6 ? `${+(amount / 1e6).toFixed(1)}M` : amount >= 1e3 ? `${+(amount / 1e3).toFixed(1)}k` : String(amount);
	return `${short} tokens`;
}

/** The currency of a few regions; anywhere else is shown in US dollars, the currency the bills are in. */
const CURRENCY_OF: Record<string, string> = {
	US: "USD",
	CA: "CAD",
	GB: "GBP",
	AU: "AUD",
	NZ: "NZD",
	JP: "JPY",
	CN: "CNY",
	IN: "INR",
	KR: "KRW",
	CH: "CHF",
	SE: "SEK",
	NO: "NOK",
	DK: "DKK",
	PL: "PLN",
	CZ: "CZK",
	HU: "HUF",
	BR: "BRL",
	MX: "MXN",
	SG: "SGD",
	HK: "HKD",
	ZA: "ZAR",
	IL: "ILS",
	TR: "TRY",
};
const EURO = ["AT", "BE", "CY", "DE", "EE", "ES", "FI", "FR", "GR", "HR", "IE", "IT", "LT", "LU", "LV", "MT", "NL", "PT", "SI", "SK"];

/** The currency people use where this language setting is from: "en-CA" -> "CAD". */
export function currencyOf(locale: string | undefined): string {
	try {
		const region = new Intl.Locale(locale ?? "en-US").maximize().region ?? "US";
		return EURO.includes(region) ? "EUR" : (CURRENCY_OF[region] ?? "USD");
	} catch (error) {
		return "USD";
	}
}

export interface Currency {
	code: string;
	/** how many of it one US dollar buys */
	rate: number;
	/** the day the rate is from; null for the dollar itself */
	date: string | null;
}

/**
 * An amount that may not be known (a model with no price): `money` when it is, null when it is not, so that the
 * caller shows nothing at all rather than "cost not known", a placeholder or 0. The engine says "not known"
 * (null); only what is shown leaves it out.
 */
export function knownMoney(usd: number | null | undefined, currency: Currency, locale?: string): string | null {
	return usd === null || usd === undefined || !Number.isFinite(usd) ? null : money(usd, currency, locale);
}

/**
 * An amount billed in US dollars, in `currency` and always with its code: "USD 1.42", "CAD 2.02", "JPY 213".
 * Amounts under one unit keep four decimals: one call to a small model costs a fraction of a cent.
 */
export function money(usd: number, currency: Currency, locale?: string): string {
	const amount = usd * currency.rate;
	const whole = new Intl.NumberFormat(locale, { style: "currency", currency: currency.code }).resolvedOptions().maximumFractionDigits ?? 2;
	const digits = Math.abs(amount) >= 1 || amount === 0 ? whole : Math.max(whole, 4);
	return new Intl.NumberFormat(locale, {
		style: "currency",
		currency: currency.code,
		currencyDisplay: "code",
		minimumFractionDigits: digits,
		maximumFractionDigits: digits,
	})
		.format(amount)
		.replace(/ /g, " ");
}
