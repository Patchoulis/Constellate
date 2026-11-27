import { clsx, type ClassValue } from "clsx";
import { twMerge } from "tailwind-merge";

/** The app's name, in one place. */
export const APP_NAME = "Constellate";

export function cn(...inputs: ClassValue[]) {
	return twMerge(clsx(inputs));
}

export function hsl(hue: number, lightness = 66, alpha = 1): string {
	return `hsla(${(((hue % 360) + 360) % 360).toFixed(1)}, 88%, ${lightness}%, ${alpha})`;
}

/** Abbreviations that keep their own capitals in a title. */
const ACRONYMS: Record<string, string> = {
	llm: "LLM",
	llms: "LLMs",
	vlm: "VLM",
	vlms: "VLMs",
	mllm: "MLLM",
	mllms: "MLLMs",
	gnn: "GNN",
	gnns: "GNNs",
	lora: "LoRA",
	sgd: "SGD",
	rl: "RL",
	marl: "MARL",
	ai: "AI",
};

/** "time series" -> "Time Series", "llms" -> "LLMs", "vision-language" -> "Vision-Language". */
export function titleCase(text: string): string {
	return text.replace(/[A-Za-z]+/g, (word) => ACRONYMS[word.toLowerCase()] ?? word[0].toUpperCase() + word.slice(1));
}
