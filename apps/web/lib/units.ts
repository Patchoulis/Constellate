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

export const SIMILARITY_NOTE =
	"Cosine similarity of the embeddings: 1 means the same direction (as alike as two texts can be), 0 means unrelated.";

/** "12%" of something named by the caller. */
export function percent(share: number): string {
	return `${Math.round(100 * share)}%`;
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
