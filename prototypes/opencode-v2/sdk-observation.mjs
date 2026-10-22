// Explicitly provisioned private SDK constructor only. Not a CLI context hook.
import { createReadOnlyTransport } from "./bounded-transport.mjs"

export function observationFetch({ baseUrl, headers, fetch, deadlineMs }) {
  if (typeof fetch !== "function") throw new Error("observation_fetch_required")
  const base = new URL(baseUrl)
  if (!base.pathname.endsWith("/")) base.pathname += "/"
  const expected = [...new Headers(headers)].sort()
  const transport = createReadOnlyTransport({ baseUrl, headers, deadlineMs, fetch: async (url, init) => {
    const response = await fetch(url, init)
    // These metadata GETs require HTTP 200. Do not normalize a different 2xx
    // into success merely because the text-only bounded transport accepts it.
    if (response.ok && response.status !== 200) {
      await response.body?.cancel()
      throw new Error("observation_status")
    }
    return response
  } })
  let invocations = 0
  return Object.freeze({
    async fetch(input, init) {
      invocations++
      const url = new URL(input)
      if (init?.method !== "GET" || init.body !== undefined || url.origin !== base.origin ||
          !url.pathname.startsWith(base.pathname) || url.username || url.password || url.hash ||
          JSON.stringify([...new Headers(init.headers)].sort()) !== JSON.stringify(expected))
        throw new Error("observation_request")
      const text = await transport.get(url.pathname.slice(base.pathname.length), {
        query: url.searchParams, signal: init.signal, outcome: true,
      })
      // The owned bounded fetch finishes before SDK response.text()/JSON.parse.
      // Errors reject without exposing or parsing upstream error bodies.
      return new Response(text, { headers: { "content-type": "application/json" } })
    },
    metrics: () => ({ invocations, ...transport.metrics() }),
    dispose: () => transport.dispose(),
  })
}
