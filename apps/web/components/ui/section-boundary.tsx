"use client";

import { Component, type ReactNode } from "react";
import { RotateCcw } from "lucide-react";
import { Button } from "@/components/ui/button";
import { changedKeys, SECTION_SHOWN, sectionLines, sectionStep, type Section } from "@/lib/ui/section-boundary";
import { cn } from "@/lib/utils";

interface Props {
	/** the section, in the words a sentence starts with: "the graph", "this paper's reader" */
	what: string;
	/** what the section is drawn from: when one of these changes (the run read again), a failed section is tried again */
	keys?: readonly unknown[];
	/** the failed section's box, in place of the section's own */
	className?: string;
	children: ReactNode;
}

interface State {
	section: Section;
	/** the keys `section` is for */
	keys: readonly unknown[] | undefined;
	/** what stopped the section, caught and not yet told to `section` (React hands it over before it reads the props) */
	caught: { error: unknown } | null;
}

/**
 * One section of the window (the graph, a panel, a feature's section) that, if it stops while being drawn, says so
 * in its own place and leaves the rest of the window working (lib/ui/section-boundary.ts). While it works it adds
 * nothing to the page: its children are drawn as they are.
 */
export class SectionBoundary extends Component<Props, State> {
	state: State = { section: SECTION_SHOWN, keys: this.props.keys, caught: null };

	static getDerivedStateFromError(error: unknown): Partial<State> {
		return { caught: { error } };
	}

	// Called before every drawing, the one after a failure included: every change of `section` is made here or by a retry.
	static getDerivedStateFromProps(props: Props, state: State): Partial<State> | null {
		let section = state.section;
		if (changedKeys(state.keys, props.keys)) section = sectionStep(section, { kind: "changed" });
		if (state.caught) section = sectionStep(section, { kind: "failed", error: state.caught.error });
		return section === state.section && state.keys === props.keys ? null : { section, keys: props.keys, caught: null };
	}

	componentDidCatch(error: unknown) {
		console.error(`${this.props.what} stopped working:`, error);
	}

	private retry = () => this.setState((before) => ({ section: sectionStep(before.section, { kind: "retried" }) }));

	render() {
		const { section } = this.state;
		if (section.state === "shown") return this.props.children;
		const lines = sectionLines(section, this.props.what);
		return (
			<div role="alert" className={cn("flex flex-col items-start gap-2 p-4 text-xs text-muted-foreground", this.props.className)}>
				<p className="text-sm text-foreground">{lines.title}</p>
				<p className="[overflow-wrap:anywhere]" title="What it reported when it stopped. The whole of it is in the window's console.">
					{lines.reason}
				</p>
				{lines.again && <p>{lines.again}</p>}
				<Button variant="outline" className="h-7" onClick={this.retry}>
					<RotateCcw />
					Try again
				</Button>
			</div>
		);
	}
}
