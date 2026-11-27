"use client";

import { createContext, useCallback, useContext, useMemo, useRef, useState } from "react";
import { AlertDialog } from "radix-ui";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

/**
 * The app's one way of asking before something that cannot be taken back (deleting a run, making one again in
 * place, forgetting the model's answers, spending on a language model). A dialog of the app's own, not the
 * browser's `confirm()`: it is styled like the rest, can say more than a line of text, and is reached by the
 * keyboard. The focus starts on the safe choice ("Cancel"), Escape or a click outside is "Cancel", and when it
 * closes the focus goes back to whatever had it before (the button that asked).
 */
export interface Question<T extends string = string> {
	title: string;
	/** what will happen, in a few short paragraphs or a list */
	body: React.ReactNode;
	/** the buttons that go ahead, the one most wanted last (it is drawn as the main button) */
	choices: { value: T; label: string; title?: string }[];
	/** the button that does nothing; "Cancel" by default */
	cancel?: string;
	/** what is done cannot be undone: the main button is drawn as a warning */
	destructive?: boolean;
}

/** A yes-or-no question: one button that goes ahead. */
export interface Confirmation {
	title: string;
	body: React.ReactNode;
	/** the button that does it: "Delete", "Make it again" */
	confirm: string;
	cancel?: string;
	destructive?: boolean;
}

/** Ask, and be told which choice was made: null for Cancel, Escape, or a click outside. */
export type Choose = <T extends string>(question: Question<T>) => Promise<T | null>;
/** Ask, and be told whether the person chose to go ahead. */
export type Confirm = (asked: Confirmation) => Promise<boolean>;

const ChooseContext = createContext<Choose | null>(null);

/** The asker of the window: one dialog, shown for one question at a time (a second question waits for the first). */
export function useChoose(): Choose {
	const choose = useContext(ChooseContext);
	if (!choose) throw new Error("useChoose needs a ConfirmProvider around it");
	return choose;
}

export function useConfirm(): Confirm {
	const choose = useChoose();
	return useMemo<Confirm>(
		() => async (asked) =>
			(await choose({
				title: asked.title,
				body: asked.body,
				cancel: asked.cancel,
				destructive: asked.destructive,
				choices: [{ value: "yes", label: asked.confirm }],
			})) === "yes",
		[choose],
	);
}

export function ConfirmProvider({ children }: { children: React.ReactNode }) {
	const [asking, setAsking] = useState<Question | null>(null);
	// the questions asked and their answers, the one shown first
	const queue = useRef<{ question: Question; answer: (choice: string | null) => void }[]>([]);
	// what had the focus when the question was asked (the button that asked): it gets it back when the dialog
	// closes. Kept here because the dialog may open after a request, when the focus has moved on.
	const opener = useRef<HTMLElement | null>(null);

	const choose = useCallback(
		(question: Question) =>
			new Promise<string | null>((resolve) => {
				if (queue.current.length === 0 && document.activeElement instanceof HTMLElement && document.activeElement !== document.body)
					opener.current = document.activeElement;
				queue.current.push({ question, answer: resolve });
				if (queue.current.length === 1) setAsking(question);
			}),
		[],
	) as Choose;
	const answer = (choice: string | null) => {
		const first = queue.current.shift();
		first?.answer(choice);
		setAsking(queue.current[0]?.question ?? null);
	};

	return (
		<ChooseContext value={choose}>
			{children}
			<AlertDialog.Root open={asking !== null} onOpenChange={(open) => open || answer(null)}>
				<AlertDialog.Portal>
					<AlertDialog.Overlay className="fixed inset-0 z-[60] bg-black/60" />
					<AlertDialog.Content
						onCloseAutoFocus={(event) => {
							const back = opener.current;
							if (queue.current.length > 0 || !back?.isConnected) return; // another question follows, or the asker is gone
							event.preventDefault();
							back.focus({ preventScroll: true });
							opener.current = null;
						}}
						className="fixed inset-0 z-[70] m-auto flex h-fit max-h-[86dvh] w-[32rem] max-w-[92vw] flex-col gap-3 overflow-y-auto rounded-lg border bg-card p-4 shadow-2xl"
					>
						{asking && (
							<>
								<AlertDialog.Title className="text-sm font-semibold">{asking.title}</AlertDialog.Title>
								<AlertDialog.Description asChild>
									<div className="flex flex-col gap-2 text-xs leading-relaxed text-muted-foreground">{asking.body}</div>
								</AlertDialog.Description>
								<div className="flex flex-wrap justify-end gap-2 pt-1">
									{/* the safe choice: Radix gives it the focus when the dialog opens */}
									<AlertDialog.Cancel asChild>
										<Button variant="outline">{asking.cancel ?? "Cancel"}</Button>
									</AlertDialog.Cancel>
									{asking.choices.map((choice, index) => {
										const main = index === asking.choices.length - 1;
										return (
											<AlertDialog.Action key={choice.value} asChild>
												<Button
													variant={main ? "default" : "outline"}
													title={choice.title}
													className={cn(main && asking.destructive && "bg-destructive text-white hover:bg-destructive/85")}
													onClick={(event) => {
														event.preventDefault(); // answered here, so that the dialog closes once
														answer(choice.value);
													}}
												>
													{choice.label}
												</Button>
											</AlertDialog.Action>
										);
									})}
								</div>
							</>
						)}
					</AlertDialog.Content>
				</AlertDialog.Portal>
			</AlertDialog.Root>
		</ChooseContext>
	);
}
