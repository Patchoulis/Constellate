"use client";

import { useReducer, useRef, useState } from "react";
import { REDO_STEPS, type Settings } from "@rp/contracts";
import { SettingsDialog } from "@/components/settings-dialog";
import { Sidebar } from "@/components/sidebar";
import { ActivityLog, Notice, Workspace } from "@/components/workspace";
import { api } from "@/lib/api";
import { ChosenModelContext, chosenModelOf } from "@/lib/chosen-model";
import { serverOwned } from "@/lib/engine";
import { forgetMarks } from "@/lib/marks";
import { choosableOf, engineCatalog } from "@/lib/models";
import { NO_SAVES, saveStep, type SaveEvent, type Saves } from "@/lib/settings-saves";
import { ready, useAsync } from "@/lib/use-async";
import { useJob } from "@/lib/use-job";

/**
 * Settings as the backend holds them; a change is saved there and the latest save's answer becomes the new state.
 * A change the backend refuses is kept with its reason (lib/settings-saves.ts) and shown until that setting is
 * saved, and the settings are read again: nothing is quietly put back.
 */
function useSettings() {
	const loaded = useAsync(api.settings, []);
	const [saves, step] = useReducer((state: Saves<Settings>, event: SaveEvent<Settings>) => saveStep(state, event), NO_SAVES);
	const asked = useRef(0); // the number of the latest save, known before the reducer has run
	const settings = saves.saved ?? ready(loaded);
	const change = (changes: Partial<Settings>) => {
		const id = ++asked.current;
		step({ kind: "asked" });
		return api.saveSettings(changes).then(
			(next) => step({ kind: "saved", id, changes, settings: next }),
			(error: Error) => {
				step({ kind: "refused", id, changes, error: error.message });
				return api.settings().then(
					(now) => step({ kind: "read", id, settings: now }),
					() => undefined, // unreachable now: what was shown stays, and the refusal says why it did not change
				);
			},
		);
	};
	return { settings, change, refused: saves.refused };
}

export default function Home() {
	const runs = useAsync(api.runs, []);
	const library = useAsync(api.library, []);
	const catalog = useAsync(api.catalog, []);
	// what kind of engine this window talks to: a server decides some settings itself
	const engine = useAsync(() => api.status(0), []);
	const { settings, change, refused } = useSettings();
	// the language models the engine offers; the Python backend has no such route, and offers Claude's alone
	const models = useAsync(() => engineCatalog(api.models), []);
	const choosable = choosableOf(ready(models), ready(catalog)?.describe_models ?? [], settings?.describe_model);
	const owned = serverOwned(ready(engine));
	const [settingsOpen, setSettingsOpen] = useState(false);
	const [chosen, setChosen] = useState<string | null>(null);
	// why renaming or deleting a run did not happen, until the next try
	const [failed, setFailed] = useState<string | null>(null);
	// Counts finished runs. A run done again keeps its name, so the name alone would not show the new result.
	const [finishedRuns, setFinishedRuns] = useState(0);
	const { job, start, follow } = useJob((finished) => {
		runs.reload();
		library.reload();
		setChosen(finished);
		setFinishedRuns((count) => count + 1);
	});

	const known = ready(runs) ?? [];
	const current = chosen ?? known[0]?.name ?? null; // until one is picked, the most recent run is shown
	const running = job.state === "running";
	const log = <ActivityLog job={job} />;
	return (
		<div className="flex h-dvh">
			<Sidebar
				runs={known}
				current={current}
				running={running}
				library={ready(library)}
				choosable={choosable}
				settings={settings}
				providerOwned={owned.has("describe_provider")}
				onChange={(changes) => void change(changes)}
				catalog={ready(catalog)}
				error={
					job.error ??
					failed ??
					// until that setting is saved; while the settings dialog is open it says so beside the setting instead
					(refused && !settingsOpen ? `Not saved: ${refused.error}` : null) ??
					(runs.status === "failed" ? runs.error : null)
				}
				onSelect={setChosen}
				onRename={(name, title) => {
					setFailed(null);
					void api.rename(name, title).then(runs.reload, (error: Error) => setFailed(error.message));
				}}
				onDelete={(name) => {
					setFailed(null);
					void api.remove(name).then(
						() => {
							forgetMarks(name); // a run made later under the same name starts with none
							if (name === current) setChosen(null); // fall back to the most recent run that is left
							runs.reload();
						},
						(error: Error) => setFailed(error.message),
					);
				}}
				onStart={start}
				onRedoAll={(name) => void start({ rerun: name, redo: REDO_STEPS })}
				onOpenSettings={() => setSettingsOpen(true)}
			/>
			{current ? (
				// every estimate on screen is for the model chosen now (lib/chosen-model.ts)
				<ChosenModelContext value={chosenModelOf(settings)}>
					<Workspace key={`${current}#${finishedRuns}`} name={current} catalog={ready(catalog)} model={settings?.describe_model}>
						{log}
					</Workspace>
				</ChosenModelContext>
			) : (
				<main className="relative min-w-0 flex-1">
					<Notice>
						{running ? "Working… the graph appears when the run finishes." : "Pick a date range or a conference and press Run."}
					</Notice>
					{log}
				</main>
			)}
			<SettingsDialog
				open={settingsOpen}
				settings={settings}
				catalog={ready(catalog)}
				validation={ready(library)?.ai_validation}
				detector={ready(library)?.ai_detector}
				machine={ready(library)?.machine}
				serverOwned={owned}
				choosable={choosable}
				onOpenChange={setSettingsOpen}
				onChange={(changes) => void change(changes).then(library.reload)} // what the library reports depends on some settings
				refused={refused}
				onReadDeclarations={
					current && !running
						? () => {
								setSettingsOpen(false);
								void api.readDeclarations(current).then(follow, follow);
							}
						: undefined
				}
				onRedo={
					current && !running
						? (steps) => {
								setSettingsOpen(false); // the run's log, behind the dialog, is what to watch now
								void start({ rerun: current, redo: steps });
							}
						: undefined
				}
			/>
		</div>
	);
}
