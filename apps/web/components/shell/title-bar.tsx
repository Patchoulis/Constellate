"use client";

import { Settings as SettingsIcon } from "lucide-react";
import { Logo } from "@/components/brand";
import { Button } from "@/components/ui/button";
import { APP_NAME } from "@/lib/utils";
import { BAR_HEIGHT, barInsets } from "@/lib/shell/title-bar";

/**
 * The window's own top bar, in place of the system's title bar (apps/desktop/main.cjs hides that one and leaves
 * only the system's minimise, maximise and close buttons, laid over the right of this bar; on a Mac, the three
 * lights over its left). The bar is what the window is dragged by; its buttons are not. In a browser tab, where
 * there is no such overlay, it is simply the app's name over the page.
 */
export function TitleBar({ onOpenSettings }: { onOpenSettings: () => void }) {
	return (
		<header
			className="flex shrink-0 items-center justify-between gap-3 border-b bg-card select-none"
			style={{ height: BAR_HEIGHT, ...barInsets(), WebkitAppRegion: "drag" } as React.CSSProperties}
		>
			<span className="flex min-w-0 items-center gap-2 text-sm font-semibold tracking-tight">
				<Logo />
				<span className="truncate">{APP_NAME}</span>
			</span>
			<span className="flex shrink-0 items-center pr-1" style={{ WebkitAppRegion: "no-drag" } as React.CSSProperties}>
				<Button variant="ghost" size="icon" title="Settings" onClick={onOpenSettings}>
					<SettingsIcon />
				</Button>
			</span>
		</header>
	);
}
