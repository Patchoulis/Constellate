import type { Metadata } from "next";
import { connection } from "next/server";
import { JetBrains_Mono, Source_Sans_3 } from "next/font/google";
import "katex/dist/katex.min.css";
import "./globals.css";
import { ConfirmProvider } from "@/components/ui/confirm";
import { APP_NAME } from "@/lib/utils";

// Chrome = JetBrains Mono, reading text (abstracts, descriptions) = Source Sans 3.
const ui = JetBrains_Mono({ subsets: ["latin"], variable: "--font-jetbrains" });
const prose = Source_Sans_3({ subsets: ["latin"], variable: "--font-sans3" });

export const metadata: Metadata = {
	title: APP_NAME,
	description: "Papers, embedded and split into communities by Fiedler vectors.",
};

export default async function RootLayout({ children }: { children: React.ReactNode }) {
	// rendered for each request, so that Next puts the nonce of the page's policy (proxy.ts) on its scripts: a page
	// made once at build time would carry none, and the policy would refuse its scripts
	await connection();
	return (
		<html lang="en" className={`dark ${ui.variable} ${prose.variable}`}>
			<body className="h-dvh overflow-hidden antialiased">
				<ConfirmProvider>{children}</ConfirmProvider>
			</body>
		</html>
	);
}
