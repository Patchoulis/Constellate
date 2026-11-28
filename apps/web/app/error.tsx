"use client";

// The page stopped while drawing: the window's own words for it, inside the app's layout (lib/shell/page-error.ts).
import { PageError } from "@/components/shell/page-error";

export default function ErrorPage({ error, reset }: { error: Error & { digest?: string }; reset: () => void }) {
	return <PageError error={error} reset={reset} />;
}
