"use client";

import { useReducer, useRef, useState } from "react";
import { REDO_STEPS, type RunRequest, type Settings } from "@rp/contracts";
import { useAskNaming, useAskRun } from "@/components/run-dialogs";
import { SettingsDialog } from "@/components/settings-dialog";
import { Sidebar } from "@/components/sidebar";
import { ActivityLog, Notice, Workspace } from "@/components/workspace";
import { api } from "@/lib/api";
import { ChosenModelContext, chosenModelOf } from "@/lib/chosen-model";
import { serverOwned } from "@/lib/engine";
import { forgetMarks } from "@/lib/marks";
import { choosableOf } from "@/lib/models";
import { namingRequest } from "@/lib/features/writing/naming";
import { NO_SAVES, saveStep, type SaveEvent, type Saves } from "@/lib/settings-saves";
import { ready, useAsync } from "@/lib/use-async";
import { useJob } from "@/lib/use-job";

/** Said when a run is made again without naming its communities again. */
const NAMES_KEPT = "The communities were not named again: their names are kept as they are.";

/**
 * Settings as the engine holds them; a change is saved there and the latest save's answer becomes the new state.
 * A change the engine refuses is kept with its reason (lib/settings-saves.ts) and shown until that setting is
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
	// the language models the engine offers; until it has told them, the settings' model alone is shown
	const models = useAsync(api.models, []);
	const choosable = choosableOf(
		ready(models),
		ready(catalog)?.describe_models ?? [],
		settings,
		models.status === "failed" ? models.error : null,
	);
	const owned = serverOwned(ready(engine));
	const [settingsOpen, setSettingsOpen] = useState(false);
	const [chosen, setChosen] = useState<string | null>(null);
	// why renaming or deleting a run did not happen, until the next try
	const [failed, setFailed] = useState<string | null>(null);
	// what came of the last start that the person should know, until the next one ("the names are kept as they are")
	const [told, setTold] = useState<string | null>(null);
	// Counts finished runs. A run done again keeps its name, so the name alone would not show the new result.
	const [finishedRuns, setFinishedRuns] = useState(0);
	const askRun = useAskRun(ready(catalog));
	const askNaming = useAskNaming();
	const {
		job,
		start: startJob,
		follow,
	} = useJob((finished) => {
		runs.reload();
		library.reload();
		setChosen(finished);
		setFinishedRuns((count) => count + 1);
	}, askRun);
	// a start being asked about or sent: nothing else is started meanwhile (a second click would queue a second
	// question about the same run, answered on what was true before the first)
	const [starting, setStarting] = useState(false);
	const start = (request: RunRequest) => {
		if (starting) return;
		setStarting(true);
		setTold(null);
		setFailed(null);
		void startJob(request)
			.then((started) => {
				if (started?.kind === "started" && started.namesKept) setTold(NAMES_KEPT);
			})
			.finally(() => setStarting(false));
	};
	/** Name the open run's communities again, at the cost shown first: the run itself is not made again. */
	const describeAgain = async (name: string) => {
		if (starting) return;
		setStarting(true);
		setTold(null);
		setFailed(null);
		try {
			const estimate = await api.describeEstimate(name, true);
			const title = ready(runs)?.find((run) => run.name === name)?.title ?? name;
			if (!(await askNaming(estimate, true, title))) return;
			await api.describeRun(name, namingRequest(estimate, true));
			follow();
		} catch (error) {
			setFailed((error as Error).message);
		} finally {
			setStarting(false);
		}
	};

	const known = ready(runs) ?? [];
	const current = chosen ?? known[0]?.name ?? null; // until one is picked, the most recent run is shown
	const running = job.state === "running";
	const busy = running || starting;
	const log = <ActivityLog job={job} />;
	return (
		<div className="flex h-dvh">
			<Sidebar
				runs={known}
				current={current}
				running={running} // a start being asked about is refused in `start`, not disabled: its button gets the focus back
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
				onRedoAll={(name) => start({ rerun: name, redo: REDO_STEPS })}
				told={told}
				warnings={ready(engine)?.warnings}
				onOpenSettings={() => setSettingsOpen(true)}
			/>
			{current ? (
				// every estimate on screen is for the model chosen now (lib/chosen-model.ts)
				<ChosenModelContext value={chosenModelOf(settings)}>
					<Workspace
						key={`${current}#${finishedRuns}`}
						name={current}
						title={known.find((run) => run.name === current)?.title ?? current}
						catalog={ready(catalog)}
						model={settings?.describe_model}
					>
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
				importProblems={ready(library)?.import_problems}
				onDetectorChanged={library.reload}
				onReadDeclarations={
					current && !busy
						? () => {
								setSettingsOpen(false);
								void api.readDeclarations(current).then(follow, (error: Error) => {
									setFailed(error.message);
									follow(); // refused as busy: what is running is shown
								});
							}
						: undefined
				}
				onRedo={
					current && !busy
						? (steps) => {
								setSettingsOpen(false); // the run's log, behind the dialog, is what to watch now
								// naming again alone does not make the run again: its communities stay as they are
								if (steps.length === 1 && steps[0] === "descriptions") void describeAgain(current);
								else start({ rerun: current, redo: steps });
							}
						: undefined
				}
			/>
		</div>
	);
}
