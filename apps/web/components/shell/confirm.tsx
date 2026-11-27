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

/** How long a question is shown before a click on it counts, and how long after one is answered the next opens. */
const READ_MS = 300;

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
	// the question shown, numbered (a new number is a new dialog: its focus starts on Cancel again), and when it was
	// shown: a click that comes sooner than a person could have read it (a double click meant for the question
	// before) is not taken as an answer
	const [asking, setAsking] = useState<{ question: Question; number: number; at: number } | null>(null);
	// the questions asked and their answers, the one shown first
	const queue = useRef<{ question: Question; answer: (choice: string | null) => void }[]>([]);
	const shown = useRef(0);
	// what had the focus when the question was asked (the button that asked): it gets it back when the dialog
	// closes. Kept here because the dialog may open after a request, when the focus has moved on.
	const opener = useRef<HTMLElement | null>(null);

	const show = useCallback(() => {
		const next = queue.current[0];
		shown.current += 1;
		setAsking(next ? { question: next.question, number: shown.current, at: Date.now() } : null);
	}, []);
	const choose = useCallback(
		(question: Question) =>
			new Promise<string | null>((resolve) => {
				if (queue.current.length === 0 && document.activeElement instanceof HTMLElement && document.activeElement !== document.body)
					opener.current = document.activeElement;
				queue.current.push({ question, answer: resolve });
				if (queue.current.length === 1) show();
			}),
		[show],
	) as Choose;
	const answer = (choice: string | null) => {
		const first = queue.current.shift();
		first?.answer(choice);
		setAsking(null);
		// the next question, if one waits, opens as a dialog of its own a moment later, never under the click that
		// answered this one
		if (queue.current.length > 0) setTimeout(show, READ_MS);
	};

	const question = asking?.question;
	return (
		<ChooseContext value={choose}>
			{children}
			<AlertDialog.Root open={asking !== null} onOpenChange={(open) => open || answer(null)}>
				<AlertDialog.Portal>
					<AlertDialog.Overlay className="fixed inset-0 z-[60] bg-black/60" />
					<AlertDialog.Content
						key={asking?.number}
						onCloseAutoFocus={(event) => {
							const back = opener.current;
							if (queue.current.length > 0 || !back?.isConnected) return; // another question follows, or the asker is gone
							event.preventDefault();
							back.focus({ preventScroll: true });
							opener.current = null;
						}}
						className="fixed inset-0 z-[70] m-auto flex h-fit max-h-[86dvh] w-[32rem] max-w-[92vw] flex-col gap-3 overflow-y-auto rounded-lg border bg-card p-4 shadow-2xl"
					>
						{question && (
							<>
								<AlertDialog.Title className="text-sm font-semibold">{question.title}</AlertDialog.Title>
								<AlertDialog.Description asChild>
									<div className="flex flex-col gap-2 text-xs leading-relaxed text-muted-foreground">{question.body}</div>
								</AlertDialog.Description>
								<div className="flex flex-wrap justify-end gap-2 pt-1">
									{/* the safe choice: Radix gives it the focus when the dialog opens */}
									<AlertDialog.Cancel asChild>
										<Button variant="outline">{question.cancel ?? "Cancel"}</Button>
									</AlertDialog.Cancel>
									{question.choices.map((choice, index) => {
										const main = index === question.choices.length - 1;
										return (
											<AlertDialog.Action key={choice.value} asChild>
												<Button
													variant={main ? "default" : "outline"}
													title={choice.title}
													className={cn(main && question.destructive && "bg-destructive text-white hover:bg-destructive/85")}
													onClick={(event) => {
														event.preventDefault(); // answered here, so that the dialog closes once
														if (asking && Date.now() - asking.at < READ_MS) return; // too soon to have been read
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
