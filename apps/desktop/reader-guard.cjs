/**
 * The embedded reader's confinement, applied to Electron. reader.cjs says what a page in the reader may open and
 * reach; this file makes Electron hold every page in the reader to it. The app (main.cjs) and the test that runs
 * a hostile page inside a real Electron (reader.electron.cjs) both use the guard made here, so what is tested is
 * what runs.
 *
 * A page in the reader is somebody else's code. It gets:
 *   a process of its own in Chromium's sandbox, with no Node, no preload script and no bridge to this app;
 *   storage of its own (the "reader" partition, kept in memory only), sharing nothing with the app's window;
 *   the network only as far as reader.cjs's lists go: every request it makes, of every kind, is checked, and
 *   every connection that is no request (a call between browsers' relay, WebTransport) is sent nowhere unless it
 *   is to a site on the lists (the session's proxy rules);
 *   no permission (camera, location, notifications, devices, screen capture, another program by its scheme...),
 *   no download, no pop-up, no dialog, no service worker that outlives it, no certificate or password of the
 *   user's, no way to keep the window from closing or to steer the app's window, and no say over where the view
 *   itself goes beyond the paper sites.
 *
 * A link the user follows inside the reader to a site on no list is not followed silently and not refused
 * silently: the user is asked, told that it is not one of the paper sites and that parts of it may not work, and
 * chooses the reader (the same sandbox, and from then on that one site more), their own browser, or neither. A
 * page cannot answer for them, and cannot make them answer: the question comes only right after the user's own
 * click or key in the view, one at a time, and a site the user said no to is not asked about again while the
 * app runs. A page sending the view elsewhere by itself is just stopped.
 */
const { READER, readable, reachable, askable, proxyRules } = require("./reader.cjs");

/** The built-in viewer of a PDF: an extension inside Chromium, and the shared pieces its page is made of. */
const PDF_VIEWER = ["chrome-extension://mhjfbmdgcfjbbpaeojofohoefgiehjai/", "chrome://resources/"];
/** Schemes that never leave the page's own process: data it made itself, and an empty frame. */
const OWN = ["data:", "blob:", "about:"];

/**
 * Everything an embedded view is given, whatever the page embedding it asks for. A page in it has its own
 * sandboxed process and no Node, preload or embedded views of its own; the built-in PDF viewer ("plugins"); no
 * pop-up of its own (a link for a new window goes through setWindowOpenHandler); and no alert, confirm or prompt,
 * which would be the app's own dialogs, titled and shown as the app's, saying whatever the page likes.
 */
const VIEW = Object.freeze({
	partition: READER,
	sandbox: true,
	contextIsolation: true,
	nodeIntegration: false,
	nodeIntegrationInSubFrames: false,
	nodeIntegrationInWorker: false,
	webSecurity: true,
	allowRunningInsecureContent: false,
	experimentalFeatures: false,
	webviewTag: false,
	navigateOnDragDrop: false,
	plugins: true,
	disablePopups: true,
	disableDialogs: true,
	enableWebSQL: false,
	spellcheck: false,
	devTools: false,
});

/** What counts as the user acting in a view: a press of a button or a key, not the mouse passing over it. */
const USER_INPUT = new Set(["mouseDown", "mouseUp", "rawKeyDown", "keyDown", "char", "gestureTap", "touchEnd"]);
/** How long after it the user's act still lets the view ask one question. */
const ACTED_MS = 3000;

/**
 * The app as a whole, before it is ready: no socket of Chromium's listens on or is bound to anything but this
 * computer's loopback. Calls between browsers (WebRTC) use no UDP and no relay but through a proxy, in every
 * page, the app's own window too (which makes none); and their local addresses are not hidden behind made-up
 * names, which is done by answering for those names on the local network (mDNS, UDP 5353 on every interface:
 * what makes Windows ask whether "Electron" may use the network). Without UDP no local address is offered to a
 * page anyway (the reader's test sees no candidate at all). Neither the app nor the reader needs either.
 */
function quietNetwork(app) {
	app.commandLine.appendSwitch("force-webrtc-ip-handling-policy", "disable_non_proxied_udp");
	app.commandLine.appendSwitch("disable-features", "WebRtcHideLocalIpsWithMdns");
}

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
 *   refusedPermission(...)  optional: told each permission refused, and its details
 */
function readerGuard({ ask, openOutside, refusedRequest, refusedPermission }) {
	/** Sites on no list that the user chose to open in the reader, for as long as the app runs. */
	const approved = new Set();
	/** Sites the user said no to: not asked about again. */
	const declined = new Set();
	let asking = false;
	/** The reader's session, once confined; and its proxy rules being set, for the lists as they are now. */
	let confined = null;
	let routed = Promise.resolve();

	/** Every connection of the reader's, requests or not, goes only where the lists say (reader.cjs proxyRules). */
	function route() {
		routed = confined.setProxy(proxyRules(approved)).then(() => confined.closeAllConnections());
		return routed;
	}

	/** Hold the reader's session (every page shown in the reader, now and later) to the rules. Call once. */
	function confineSession(session) {
		const reader = session.fromPartition(READER);
		confined = reader;
		void route();
		// Every request a page makes, whatever it is for: a frame, a script's fetch, an image, a form, a socket.
		// Only a page that is shown makes requests: one made for no page at all (no webContents) is a service
		// worker's (its script, its imports, its own fetches) or a shared worker's, which outlive the page that
		// started them and act for its site's later pages. Refused, so nothing a page leaves behind acts after it
		// is gone: without its script there is no worker.
		reader.webRequest.onBeforeRequest((details, answer) => {
			const shown = details.webContentsId !== undefined;
			const allowed = shown && (internal(details.url) || reachable(details.url, details.resourceType, approved));
			if (!allowed) refusedRequest?.(details.url, shown ? details.resourceType : `${details.resourceType} (for no page)`);
			answer({ cancel: !allowed });
		});
		// nothing a page asks for is granted, and nothing is said to be granted when it only asks whether
		// (this includes "openExternal": a link with a scheme of another program's, ms-msdt: or a custom one)
		reader.setPermissionRequestHandler((_contents, permission, answer, details) => {
			refusedPermission?.(permission, details);
			answer(false);
		});
		reader.setPermissionCheckHandler(() => false);
		reader.setDevicePermissionHandler(() => false);
		reader.setDisplayMediaRequestHandler((_request, answer) => answer({}));
		// a page saves nothing onto this computer
		reader.on("will-download", (event) => event.preventDefault());
		return reader;
	}

	/**
	 * The view is being sent to a page on no list: stop it, and let the user decide, once. Only right after the
	 * user's own click or key in the view (`acted` says whether there was one, and uses it up): a page that sends
	 * the view elsewhere by itself is simply stopped, so it can neither make the question appear nor make it
	 * appear again and again, one made-up site after another.
	 */
	function consider(guest, target, acted) {
		const host = askable(target);
		if (host === null || declined.has(host) || asking || !acted()) return;
		asking = true;
		Promise.resolve(ask(host, target))
			.then((answer) => {
				if (answer === "reader") {
					approved.add(host);
					return route().then(() => {
						if (!guest.isDestroyed()) void guest.loadURL(target).catch(() => {});
					});
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
			// Whatever the page that embeds the view asked for (a preload, Node, Blink features, pop-ups, another
			// partition...), the view gets exactly these settings and nothing else: none is kept from the request
			// but its zoom.
			const zoomFactor = preferences.zoomFactor;
			for (const name of Object.keys(preferences)) delete preferences[name];
			Object.assign(preferences, VIEW, zoomFactor === undefined ? {} : { zoomFactor });
			if (!readable(params.src, approved) || params.partition !== READER) event.preventDefault();
		});
		window.webContents.on("did-attach-webview", (_event, guest) => {
			// the user's own last click or key in the view: what lets the view ask about a site on no list, once
			let input = -Infinity;
			guest.on("input-event", (_input, { type }) => {
				if (USER_INPUT.has(type)) input = Date.now();
			});
			const acted = () => {
				const fresh = Date.now() - input < ACTED_MS;
				input = -Infinity;
				return fresh;
			};
			// The view itself only goes to the paper sites and to what the user approved. A page cannot send the
			// view elsewhere by navigating or by being redirected: the user is asked (consider, above).
			const stay = (event, target) => {
				if (event.isMainFrame === false || readable(target, approved)) return;
				event.preventDefault();
				consider(guest, target, acted);
			};
			guest.on("will-navigate", stay);
			guest.on("will-redirect", stay);
			// a frame inside a shown page: the same rule as every other request it makes
			guest.on("will-frame-navigate", (event) => {
				if (!event.isMainFrame && !internal(event.url) && !reachable(event.url, "subFrame", approved)) event.preventDefault();
			});
			// Nothing opens a window of this app. A paper site's link opened in a new window goes to the user's
			// browser; any other is asked about like a link followed in the view (and only after the user's click).
			guest.setWindowOpenHandler(({ url: target }) => {
				if (readable(target, approved)) openOutside(target);
				else consider(guest, target, acted);
				return { action: "deny" };
			});
			// A page asking for a name and password, or for a certificate of the user's, is given neither: the
			// callback without one cancels. (Left alone, Electron would hand over the first certificate there is.)
			guest.on("login", (event, _details, _info, callback) => {
				event.preventDefault();
				callback();
			});
			guest.on("select-client-certificate", (event, _url, _certificates, callback) => {
				event.preventDefault();
				callback();
			});
			// a page asking not to be left ("leave site?") cannot keep the view, or the window around it, open
			guest.on("will-prevent-unload", (event) => event.preventDefault());
			// calls between browsers go around the request check: no UDP, no relay but through a proxy (and the app
			// as a whole is held to the same: quietNetwork, below)
			guest.setWebRTCIPHandlingPolicy("disable_non_proxied_udp");
		});
	}

	return { confineSession, guardWindow, approved, declined, routed: () => routed };
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

/** Do two addresses have the same scheme, host and port? */
function sameOrigin(one, other) {
	try {
		return new URL(one).origin === new URL(other).origin;
	} catch {
		return false;
	}
}

/**
 * The app's own window (at `home`) shows the app and nothing else, whatever it is told. Not its page: that only
 * goes to the app's own addresses. Not a frame of its page either: the Paper tab shows a paper's PDF in a frame,
 * and a link in a PDF is its authors' to write; followed there, somebody else's page would be shown inside the
 * app's window with none of the reader's confinement (in the app's own session, which reaches this computer). It
 * goes to the user's browser, as does a link opened in a new window. Nothing opens a second window of this app.
 */
function holdAppWindow(window, home, openOutside) {
	const contents = window.webContents;
	contents.on("will-navigate", (event, target) => {
		if (!sameOrigin(target, home)) event.preventDefault();
	});
	contents.on("will-frame-navigate", (event) => {
		if (event.isMainFrame || sameOrigin(event.url, home) || internal(event.url)) return;
		event.preventDefault();
		if (/^https:\/\//.test(event.url)) openOutside(event.url);
	});
	contents.setWindowOpenHandler(({ url: target }) => {
		if (/^https:\/\//.test(target)) openOutside(target);
		return { action: "deny" };
	});
}

module.exports = { readerGuard, holdAppWindow, question, internal, quietNetwork, VIEW };
