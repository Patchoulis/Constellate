/**
 * How this person likes the interface to behave: choices that are about the window, not about a run or the
 * engine's work (those are the engine's Settings).
 *
 * Kept in this browser's localStorage under one key, as `{"version":1,"prefs":{...}}`. The core is pure; `usePrefs`
 * holds the choices in React, shared by everything on the page.
 */
import { useEffect, useState, useSyncExternalStore } from "react";

/**
 * The key held to select papers on the graph: with it down, a click marks a paper and a drag draws a lasso.
 * "primary" is Ctrl on Windows and Linux and Command on a Mac (where Ctrl with a click is the right click).
 * Every one of the three exists on every keyboard and system.
 */
export type SelectKey = "primary" | "shift" | "alt";

export interface Prefs {
	selectKey: SelectKey;
	/** whether the marked papers' rings, links and names are drawn on the graph */
	marksShown: boolean;
	/** marking a paper while the marks are hidden shows them again */
	showMarksOnMark: boolean;
}

export const INITIAL_PREFS: Prefs = { selectKey: "primary", marksShown: true, showMarksOnMark: true };

export const SELECT_KEYS: readonly SelectKey[] = ["primary", "shift", "alt"];

/** Whether this is a Mac's keyboard, as far as the browser says: only the key's name depends on it. */
export function onMac(platform: string): boolean {
	return /mac|iphone|ipad/i.test(platform);
}

/** A select key as it is written on the keyboard in front of the person. */
export function selectKeyName(key: SelectKey, mac: boolean): string {
	if (key === "primary") return mac ? "⌘" : "Ctrl";
	if (key === "alt") return mac ? "⌥" : "Alt";
	return "Shift";
}

/** Whether the select key is down in a pointer or key event. "primary" takes either Ctrl or Command, so it need
 *  not know which system it is on. */
export function selectKeyHeld(key: SelectKey, event: { ctrlKey: boolean; metaKey: boolean; shiftKey: boolean; altKey: boolean }): boolean {
	if (key === "primary") return event.ctrlKey || event.metaKey;
	return key === "shift" ? event.shiftKey : event.altKey;
}

export const PREFS_KEY = "constellate.prefs";
const VERSION = 1;

export function serialisePrefs(prefs: Prefs): string {
	return JSON.stringify({ version: VERSION, prefs });
}

/** Whatever storage holds, read as the choices: anything that cannot be trusted gives the initial choice for
 *  that one. Never an error. */
export function parsePrefs(text: string | null | undefined): Prefs {
	if (!text) return INITIAL_PREFS;
	let stored: unknown;
	try {
		stored = JSON.parse(text);
	} catch (error) {
		return INITIAL_PREFS;
	}
	if (typeof stored !== "object" || stored === null) return INITIAL_PREFS;
	const { version, prefs } = stored as { version?: unknown; prefs?: unknown };
	if (version !== VERSION || typeof prefs !== "object" || prefs === null) return INITIAL_PREFS;
	const kept = prefs as Record<string, unknown>;
	const flag = (name: "marksShown" | "showMarksOnMark") => (typeof kept[name] === "boolean" ? kept[name] : INITIAL_PREFS[name]);
	return {
		selectKey: SELECT_KEYS.includes(kept.selectKey as SelectKey) ? (kept.selectKey as SelectKey) : INITIAL_PREFS.selectKey,
		marksShown: flag("marksShown"),
		showMarksOnMark: flag("showMarksOnMark"),
	};
}

let current: Prefs | null = null;
const listeners = new Set<() => void>();

function snapshot(): Prefs {
	if (current) return current;
	try {
		current = parsePrefs(window.localStorage.getItem(PREFS_KEY));
	} catch (error) {
		current = INITIAL_PREFS; // storage cannot be read (a private window, a disabled store)
	}
	return current;
}

function subscribe(listener: () => void) {
	listeners.add(listener);
	return () => void listeners.delete(listener);
}

/** Change some of the choices: shown at once everywhere, and remembered. */
export function setPrefs(change: Partial<Prefs>) {
	const next = { ...snapshot(), ...change };
	current = next;
	try {
		window.localStorage.setItem(PREFS_KEY, serialisePrefs(next));
	} catch (error) {
		// storage is full or switched off: the choices still hold until the window is closed
	}
	for (const listener of listeners) listener();
}

/** The select key's name on the keyboard in front of the person. Until the page is in a window, as on a PC. */
export function useSelectKeyName(key: SelectKey): string {
	const [mac, setMac] = useState(false);
	useEffect(() => setMac(onMac(window.navigator.platform)), []);
	return selectKeyName(key, mac);
}

/** The choices now. Where the page is rendered without a window, the initial ones. */
export function usePrefs(): Prefs {
	return useSyncExternalStore(subscribe, snapshot, () => INITIAL_PREFS);
}
