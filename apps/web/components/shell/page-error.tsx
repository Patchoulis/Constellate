"use client";

import { useEffect } from "react";
import { errorLine, isStaleInterface, PAGE_ERROR } from "@/lib/shell/page-error";

/**
 * The window's own screen for a page that stopped working, in place of the framework's bare one: what happened in
 * plain words, the reason as far as it is known, and two ways on. Plain elements and inline styles for the few
 * things the stylesheet may not be there for (the whole page can be what failed to load).
 */
export function PageError({ error, reset }: { error: Error & { digest?: string }; reset?: () => void }) {
	useEffect(() => {
		console.error("The page stopped working:", error);
	}, [error]);
	const stale = isStaleInterface(error);
	const button = "h-8 rounded-md border px-3 text-sm";
	return (
		<main
			role="alert"
			className="flex h-dvh w-full items-center justify-center p-8"
			style={{ background: "#0d0d0f", color: "#e8e8e8", fontFamily: "ui-monospace, monospace" }}
		>
			<div className="flex max-w-xl flex-col gap-3" style={{ maxWidth: "36rem" }}>
				<h1 className="text-base font-semibold" style={{ fontSize: "1rem", fontWeight: 600, margin: 0 }}>
					{PAGE_ERROR.title}
				</h1>
				<p className="text-sm leading-relaxed" style={{ fontSize: "0.875rem", lineHeight: 1.6, margin: 0, opacity: 0.85 }}>
					{stale ? PAGE_ERROR.stale : PAGE_ERROR.other}
				</p>
				<p
					className="rounded-md border p-2 text-xs break-words"
					style={{ fontSize: "0.75rem", margin: 0, opacity: 0.7, overflowWrap: "anywhere" }}
					title="What the page reported when it stopped. The whole of it is in the window's console."
				>
					{errorLine(error)}
				</p>
				<div className="flex flex-wrap gap-2" style={{ display: "flex", flexWrap: "wrap", gap: "0.5rem" }}>
					{reset && !stale && (
						<button type="button" className={button} onClick={reset}>
							{PAGE_ERROR.again}
						</button>
					)}
					<button type="button" className={button} onClick={() => window.location.reload()}>
						{PAGE_ERROR.reload}
					</button>
				</div>
				<p className="text-xs" style={{ fontSize: "0.75rem", margin: 0, opacity: 0.55 }}>
					{PAGE_ERROR.log}
				</p>
			</div>
		</main>
	);
}
