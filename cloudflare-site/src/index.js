const ORIGIN = "https://duola-agentcost.pages.dev";

export default {
  async fetch(request) {
    const incoming = new URL(request.url);
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
