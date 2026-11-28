"use client";

// The layout itself stopped: this replaces the whole document, so it brings its own <html> and <body>.
import "./globals.css";
import { PageError } from "@/components/shell/page-error";

export default function GlobalErrorPage({ error, reset }: { error: Error & { digest?: string }; reset: () => void }) {
	return (
		<html lang="en" className="dark">
			<body style={{ margin: 0 }}>
				<PageError error={error} reset={reset} />
			</body>
		</html>
	);
}
