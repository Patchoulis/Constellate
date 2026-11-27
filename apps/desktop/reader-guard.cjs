/**
 * The embedded reader's confinement, applied to Electron. reader.cjs says what a page in the reader may open and
 * reach; this file makes Electron hold every page in the reader to it. The app (main.cjs) and the test that runs
 * a hostile page inside a real Electron (reader.electron.cjs) both use the guard made here, so what is tested is
 * what runs.
 *
 * A page in the reader is somebody else's code. It gets:
 *   a process of its own in Chromium's sandbox, with no Node, no preload script and no bridge to this app;
 *   storage of its own (the "reader" partition, kept in memory only), sharing nothing with the app's window;
 *   the network only as far as reader.cjs's lists go: every request it makes, of every kind, is checked;
 *   no permission (camera, location, notifications, devices, screen capture...), no download, no pop-up, no
 *   way to steer the app's window, and no say over where the view itself goes beyond the paper sites.
 *
 * A link the user follows inside the reader to a site on no list is not followed silently and not refused
 * silently: the user is asked, told that it is not one of the paper sites and that parts of it may not work, and
 * chooses the reader (the same sandbox, and from then on that one site more), their own browser, or neither. A
 * page cannot answer for them, and cannot ask again and again: one question at a time, and a site the user said
 * no to is not asked about again while the app runs.
 */
const { READER, readable, reachable, askable } = require("./reader.cjs");

/** The built-in viewer of a PDF: an extension inside Chromium, and the shared pieces its page is made of. */
const PDF_VIEWER = ["chrome-extension://mhjfbmdgcfjbbpaeojofohoefgiehjai/", "chrome://resources/"];
/** Schemes that never leave the page's own process: data it made itself, and an empty frame. */
const OWN = ["data:", "blob:", "about:"];

/**
 * Is this request one the browser makes inside itself rather than of the network? Only its own data and the PDF
 * viewer's own files: reader.cjs decides everything that names a place on a network.
 */
function internal(address) {
	return OWN.some((scheme) => address.startsWith(scheme)) || PDF_VIEWER.some((place) => address.startsWith(place));
}

/**
 * The reader's guard: one for the app.
 *
 *   ask(host, address)      asks the user about a site on no list; resolves to "reader", "browser" or "no"
 *   openOutside(address)    hands an https link to the user's own browser
 *   refusedRequest(...)     optional: told each request that was stopped (the test's eyes; a log's)
 */
function readerGuard({ ask, openOutside, refusedRequest }) {
	/** Sites on no list that the user chose to open in the reader, for as long as the app runs. */
	const approved = new Set();
	/** Sites the user said no to: not asked about again. */
	const declined = new Set();
	let asking = false;

	/** Hold the reader's session (every page shown in the reader, now and later) to the rules. Call once. */
	function confineSession(session) {
		const reader = session.fromPartition(READER);
		// every request a page makes, whatever it is for: a frame, a script's fetch, an image, a form, a socket
		reader.webRequest.onBeforeRequest((details, answer) => {
			const allowed = internal(details.url) || reachable(details.url, details.resourceType, approved);
			if (!allowed) refusedRequest?.(details.url, details.resourceType);
			answer({ cancel: !allowed });
		});
		// nothing a page asks for is granted, and nothing is said to be granted when it only asks whether
		reader.setPermissionRequestHandler((_contents, _permission, answer) => answer(false));
		reader.setPermissionCheckHandler(() => false);
		reader.setDevicePermissionHandler(() => false);
		reader.setDisplayMediaRequestHandler((_request, answer) => answer({}));
		// a page saves nothing onto this computer
		reader.on("will-download", (event) => event.preventDefault());
		return reader;
	}

	/** The view is being sent to a page on no list: stop it, and let the user decide, once. */
	function consider(guest, target) {
		const host = askable(target);
		if (host === null || declined.has(host) || asking) return;
		asking = true;
		Promise.resolve(ask(host, target))
			.then((answer) => {
				if (answer === "reader") {
					approved.add(host);
					if (!guest.isDestroyed()) void guest.loadURL(target).catch(() => {});
				} else if (answer === "browser") {
					openOutside(target);
				} else {
					declined.add(host);
				}
			})
			.catch(() => declined.add(host))
			.finally(() => {
				asking = false;
			});
	}

	/** Hold the embedded views of `window` to the rules: what may be attached, and where an attached view may go. */
	function guardWindow(window) {
		window.webContents.on("will-attach-webview", (event, preferences, params) => {
			// whatever the page that embeds the view asked for, the view gets none of this app
			delete preferences.preload;
			delete preferences.preloadURL;
			preferences.nodeIntegration = false;
			preferences.nodeIntegrationInSubFrames = false;
			preferences.nodeIntegrationInWorker = false;
			preferences.contextIsolation = true;
			preferences.sandbox = true;
			preferences.webSecurity = true;
			preferences.allowRunningInsecureContent = false;
			preferences.experimentalFeatures = false;
			preferences.webviewTag = false; // a view cannot embed a view of its own
			preferences.navigateOnDragDrop = false;
			preferences.partition = READER;
			if (!readable(params.src, approved) || params.partition !== READER) event.preventDefault();
		});
		window.webContents.on("did-attach-webview", (_event, guest) => {
			// The view itself only goes to the paper sites and to what the user approved. A page cannot send the
			// view elsewhere by navigating or by being redirected: the user is asked (consider, above).
			const stay = (event, target) => {
				if (event.isMainFrame === false || readable(target, approved)) return;
				event.preventDefault();
				consider(guest, target);
			};
			guest.on("will-navigate", stay);
			guest.on("will-redirect", stay);
			// a frame inside a shown page: the same rule as every other request it makes
			guest.on("will-frame-navigate", (event) => {
				if (!event.isMainFrame && !internal(event.url) && !reachable(event.url, "subFrame", approved)) event.preventDefault();
			});
			// a link opened in a new window goes to the user's browser; nothing opens a window of this app
			guest.setWindowOpenHandler(({ url: target }) => {
				if (/^https:\/\//.test(target)) openOutside(target);
				return { action: "deny" };
			});
			// a page asking for a name and password, or for a certificate of the user's, is given neither
			guest.on("login", (event) => event.preventDefault());
			guest.on("select-client-certificate", (event) => event.preventDefault());
			// calls between browsers go around the request check: they are kept from finding this computer's addresses
			guest.setWebRTCIPHandlingPolicy("disable_non_proxied_udp");
		});
	}

	return { confineSession, guardWindow, approved, declined };
}

/** What the user is asked about a site on no list: the words of the question, for the app's dialog. */
function question(host) {
	return {
		message: `${host} is not one of the paper sites Constellate knows.`,
		detail:
			"The reader normally opens only arXiv, OpenReview and the conferences' own proceedings. " +
			`If you open ${host} in the reader, it runs in the same sandbox as every other page (cut off from this app and ` +
			"from this computer), but it is not a site Constellate has been checked against: parts of it may not load or " +
			"may not work, and you should trust it no more than any other link.",
		buttons: ["Cancel", "Open in my browser", "Open in the reader"],
		answers: ["no", "browser", "reader"],
	};
}

module.exports = { readerGuard, question, internal };
