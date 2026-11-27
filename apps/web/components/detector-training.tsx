"use client";

import { useState } from "react";
import { CalendarSearch, GraduationCap, Loader2, Square } from "lucide-react";
import type { DetectorEstimate, DetectorState } from "@rp/contracts";
import { Button } from "@/components/ui/button";
import { api } from "@/lib/api";
import { accepting, costTitle, DOLLAR, shownModel } from "@/lib/cost";
import { detectorEstimateLines, trainingLine } from "@/lib/detector";
import { elsewhereLine, useFollowedJob } from "@/lib/use-followed-job";

/** Where the estimate stands: not asked, being asked, shown (to be accepted with the click), or refused. */
type Estimating = { kind: "idle" } | { kind: "asking" } | { kind: "shown"; estimate: DetectorEstimate } | { kind: "failed"; error: string };

/**
 * Training the AI-text detector, which asks the chosen model to write abstracts it learns to tell from people's,
 * and so costs money: what it would ask and cost is shown first (`GET /api/detector/estimate`), and the build is
 * started with what was shown (`POST /api/detector/build`), as a job with its progress and a way to stop it. The
 * check by year asks arXiv only and is free.
 */
export function DetectorTraining({ state, onChanged }: { state: DetectorState; onChanged: () => void }) {
	const [estimating, setEstimating] = useState<Estimating>({ kind: "idle" });
	const building = useFollowedJob(() => {
		setEstimating({ kind: "idle" });
		onChanged();
	}, "detector:build");
	const years = useFollowedJob(onChanged, "detector:years");
	const busy = building.job.state === "starting" || building.job.state === "running" || years.job.state === "running";
	const other = busy ? null : (building.elsewhere ?? years.elsewhere);

	const estimate = () => {
		setEstimating({ kind: "asking" });
		api.detectorEstimate().then(
			(estimate) => setEstimating({ kind: "shown", estimate }),
			(error: Error) => setEstimating({ kind: "failed", error: error.message }),
		);
	};
	const train = (estimate: DetectorEstimate) =>
		void building.start(async () => {
			// the model and the most shown go with the click: the engine refuses if either changed since
			const started = await api.buildDetector({ ...shownModel(estimate.cost), ...accepting(estimate.cost) });
			return started.job;
		});

	const running = building.job.state === "running" ? building.job : years.job.state === "running" ? years.job : null;
	const runningKind = building.job.state === "running" ? "build" : "years";
	const ended = building.job.state === "failed" ? building.job.error : years.job.state === "failed" ? years.job.error : null;
	return (
		<div className="flex flex-col gap-2 rounded-md border px-3 py-2.5 text-xs">
			{running ? (
				<div className="flex items-center gap-2 text-muted-foreground" role="status">
					<Loader2 className="size-3.5 shrink-0 animate-spin" />
					<span className="flex-1">{trainingLine(runningKind, running.progress, DOLLAR)}</span>
					<Button
						variant="ghost"
						size="icon"
						title="Stop: what is under way is finished, nothing new is asked"
						onClick={() => void api.cancelJob(running.id).catch(() => undefined)}
					>
						<Square />
					</Button>
				</div>
			) : estimating.kind === "shown" ? (
				<EstimateShown
					estimate={estimating.estimate}
					disabled={busy || other !== null}
					onTrain={() => train(estimating.estimate)}
					onCancel={() => setEstimating({ kind: "idle" })}
				/>
			) : (
				<div className="flex flex-wrap items-center justify-between gap-2">
					<span className="text-muted-foreground">
						{state === "stale"
							? "Trained by an earlier version: training again uses what was written before."
							: state === "ready"
								? "Trained."
								: state === "building"
									? "Being trained."
									: "Not trained: nothing is flagged until it is."}
					</span>
					<span className="flex gap-1.5">
						{(state === "untrained" || state === "stale") && (
							<Button variant="outline" disabled={estimating.kind === "asking" || busy || other !== null} onClick={estimate}>
								{estimating.kind === "asking" ? <Loader2 className="animate-spin" /> : <GraduationCap />}
								Train the detector…
							</Button>
						)}
						{(state === "ready" || state === "stale") && (
							<Button
								variant="outline"
								disabled={busy || other !== null}
								title="Scores a week of arXiv abstracts in each year: arXiv only, no language model, no cost"
								onClick={() => void years.start(async () => (await api.checkDetectorYears()).job)}
							>
								<CalendarSearch />
								Check by year
							</Button>
						)}
					</span>
				</div>
			)}
			{estimating.kind === "failed" && (
				<p className="text-destructive">What training would cost could not be worked out: {estimating.error}</p>
			)}
			{ended && !running && <p className="text-destructive">{ended}</p>}
			{other && <p className="text-muted-foreground">{elsewhereLine(other)}</p>}
		</div>
	);
}

function EstimateShown({
	estimate,
	disabled,
	onTrain,
	onCancel,
}: {
	estimate: DetectorEstimate;
	disabled: boolean;
	onTrain: () => void;
	onCancel: () => void;
}) {
	const words = detectorEstimateLines(estimate, DOLLAR);
	return (
		<div className="flex flex-col gap-1.5">
			{words.lines.map((line) => (
				<p key={line} className="text-muted-foreground">
					{line}
				</p>
			))}
			{words.cost && (
				<p className="text-foreground" title={costTitle(estimate.cost)}>
					{words.cost}
				</p>
			)}
			{words.cannot && <p className="text-warning">{words.cannot}</p>}
			<div className="flex justify-end gap-1.5">
				<Button variant="ghost" onClick={onCancel}>
					Cancel
				</Button>
				{!words.cannot && (
					<Button disabled={disabled} title={costTitle(estimate.cost)} onClick={onTrain}>
						<GraduationCap />
						{words.free ? "Train it again" : "Train the detector"}
					</Button>
				)}
			</div>
		</div>
	);
}
