"use client";

import { useState } from "react";
import type { Settings } from "@rp/contracts";
import { SettingsDialog } from "@/components/settings-dialog";
import { Sidebar } from "@/components/sidebar";
import { ActivityLog, Notice, Workspace } from "@/components/workspace";
import { api } from "@/lib/api";
import { ready, useAsync } from "@/lib/use-async";
import { useJob } from "@/lib/use-job";

/** Settings as the backend holds them; a change is saved there and the answer becomes the new state. */
function useSettings() {
	const loaded = useAsync(api.settings, []);
	const [saved, setSaved] = useState<Settings>();
	const settings = saved ?? ready(loaded);
	const change = (changes: Partial<Settings>) => api.saveSettings(changes).then(setSaved, () => loaded.reload());
	return { settings, change };
}

export default function Home() {
	const runs = useAsync(api.runs, []);
	const library = useAsync(api.library, []);
	const catalog = useAsync(api.catalog, []);
	const { settings, change } = useSettings();
	const [settingsOpen, setSettingsOpen] = useState(false);
	const [chosen, setChosen] = useState<string | null>(null);
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
				model={settings?.describe_model}
				onModel={(describe_model) => void change({ describe_model })}
				catalog={ready(catalog)}
				error={job.error ?? (runs.status === "failed" ? runs.error : null)}
				onSelect={setChosen}
				onRename={(name, title) => void api.rename(name, title).then(runs.reload)}
				onDelete={(name) =>
					void api.remove(name).then(() => {
						if (name === current) setChosen(null); // fall back to the most recent run that is left
						runs.reload();
					})
				}
				onStart={start}
				onOpenSettings={() => setSettingsOpen(true)}
			/>
			{current ? (
				<Workspace key={`${current}#${finishedRuns}`} name={current} catalog={ready(catalog)}>
					{log}
				</Workspace>
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
				onOpenChange={setSettingsOpen}
				onChange={change}
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
						? (step) => {
								setSettingsOpen(false); // the run's log, behind the dialog, is what to watch now
								void start({ rerun: current, redo: [step] });
							}
						: undefined
				}
			/>
		</div>
	);
}
