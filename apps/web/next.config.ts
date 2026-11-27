import path from "node:path";
import type { NextConfig } from "next";

const nextConfig: NextConfig = {
	transpilePackages: ["@rp/contracts"],
	// A self-contained server (.next/standalone) that the desktop shell runs as a child process.
	output: "standalone",
	// No images are resized by the server, so the image library (and the LGPL code it brings) is not shipped.
	images: { unoptimized: true },
	outputFileTracingExcludes: { "*": ["node_modules/sharp/**", "node_modules/@img/**"] },
	outputFileTracingRoot: path.join(import.meta.dirname, "..", ".."),
	// /api and /auth are the engine's: proxy.ts forwards them, with the engine's token added on that hop alone (a
	// rewrite here could not add it). A forwarded request may wait as long as the engine takes to fetch a PDF from
	// its publisher (180 s, apps/engine/README.md) and a little more; Next's own limit is 30 s.
	experimental: { proxyTimeout: 200_000 },
	// No other site may put this interface in a frame (a page shown in the reader could otherwise try to).
	async headers() {
		return [{ source: "/:path*", headers: [{ key: "Content-Security-Policy", value: "frame-ancestors 'self'" }] }];
	},
};

export default nextConfig;
