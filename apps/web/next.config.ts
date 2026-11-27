import path from "node:path";
import type { NextConfig } from "next";

// Where the Python backend listens. Rewrites are fixed when the app is built, so the desktop shell starts the
// backend on this same port (apps/desktop/main.cjs).
const BACKEND = process.env.RP_BACKEND ?? "http://127.0.0.1:8765";

const nextConfig: NextConfig = {
	transpilePackages: ["@rp/contracts"],
	// A self-contained server (.next/standalone) that the desktop shell runs as a child process.
	output: "standalone",
	// No images are resized by the server, so the image library (and the LGPL code it brings) is not shipped.
	images: { unoptimized: true },
	outputFileTracingExcludes: { "*": ["node_modules/sharp/**", "node_modules/@img/**"] },
	outputFileTracingRoot: path.join(import.meta.dirname, "..", ".."),
	// The backend owns /api; the window only ever talks to this one origin.
	// No other site may put this interface in a frame (a page shown in the reader could otherwise try to).
	async headers() {
		return [{ source: "/:path*", headers: [{ key: "Content-Security-Policy", value: "frame-ancestors 'self'" }] }];
	},
	async rewrites() {
		return [{ source: "/api/:path*", destination: `${BACKEND}/api/:path*` }];
	},
};

export default nextConfig;
