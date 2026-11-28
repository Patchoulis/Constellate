/**
 * The window's own top bar (components/shell/title-bar.tsx): its height, shared with the shell so that the
 * system's window buttons are as tall as the bar they lie over, and the room it leaves them.
 */

/** In CSS pixels. apps/desktop/main.cjs gives the system's buttons the same height (BAR_HEIGHT there). */
export const BAR_HEIGHT = 36;

/**
 * The room the bar leaves the system's own controls: on its right for the minimise, maximise and close buttons of
 * Windows and Linux, on its left for a Mac's three lights. The browser says where the free area is
 * (`titlebar-area-*`, set only while a window's controls are laid over its page); in a browser tab there are none
 * and both come to 0.
 */
export function barInsets(): { paddingLeft: string; paddingRight: string } {
	return {
		paddingLeft: "max(1rem, env(titlebar-area-x, 0px))",
		paddingRight: "calc(100vw - env(titlebar-area-x, 0px) - env(titlebar-area-width, 100vw))",
	};
}
