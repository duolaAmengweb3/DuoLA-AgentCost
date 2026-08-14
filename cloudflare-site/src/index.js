const ORIGIN = "https://duola-agentcost.pages.dev";

export default {
  async fetch(request) {
    const incoming = new URL(request.url);
    const releasePrefix = "/downloads/duola-agentcost-v0.1.4-";
    // Keep the historical /downloads URL usable while the canonical release
    // artifacts live on GitHub. Pages' SPA fallback would otherwise return
    // HTML with a 200 status for a missing binary, which is a bad installer
    // failure mode.
    if (incoming.pathname.startsWith(releasePrefix)) {
      const filename = incoming.pathname.slice("/downloads/".length);
      if (!filename.includes("/")) {
        return Response.redirect(
          `https://github.com/duolaAmengweb3/DuoLA-AgentCost/releases/download/v0.1.4/${filename}`,
          302
        );
      }
    }
    const target = new URL(ORIGIN);
    // The public root is the product landing page. The local Dashboard
    // preview remains available at /dashboard so the two user journeys are
    // never confused.
    target.pathname = incoming.pathname === "/" || incoming.pathname === "/index.html"
      ? "/landing.html"
      : incoming.pathname === "/dashboard"
        ? "/index.html"
        : incoming.pathname;
    target.search = incoming.search;

    const headers = new Headers(request.headers);
    headers.set("x-duola-edge-preview", "1");
    headers.delete("host");

    return fetch(new Request(target, {
      method: request.method,
      headers,
      body: request.method === "GET" || request.method === "HEAD" ? undefined : request.body,
      redirect: "follow"
    }));
  }
};
