// Dependency-free OpenCode V2 CLI entrypoint. Not a server or Mini plugin.
import { LiveState, supportsVersion, unknown } from "./core.mjs"
import { LiveReporter } from "./reporter.mjs"

export function createPlugin(runtime = {}) {
  return {
    id: "verij.monitoring.v2.live",
    setup(context) {
      const options = context.options ?? {}, env = runtime.env ?? process.env
      const version = context.app?.version
      if (options.experimentalLiveStatus !== true || !supportsVersion(version)
          || !/^\d+$/.test(env.ZELLIJ_PANE_ID ?? "") || !env.ZELLIJ_SESSION_NAME
          || !options.reporter?.startsWith("/")) return
      const clock = runtime.clock ?? globalThis
      const state = new LiveState(context, () => clock.now ? clock.now() : performance.now())
      const reporter = new LiveReporter({ executable: options.reporter,
        args: ["agent", "opencode", "--session", env.ZELLIJ_SESSION_NAME,
          "--pane", env.ZELLIJ_PANE_ID, "--pid", String(process.pid)],
        env: { ...env, VERIJ_AGENT_STATE_DIR: env.VERIJ_AGENT_STATE_DIR ?? options.runtimeDir,
          VERIJ_STATES_DIR: env.VERIJ_STATES_DIR ?? options.statesDir },
        unknown: unknown(version), reset: () => state.reset(), spawn: runtime.spawn, clock })
      let disposed = false, unsubscribe, timer
      const tick = () => {
        if (disposed) return
        try {
          reporter.tick(reporter.now())
          if (reporter.ready) reporter.publish(state.sample())
        } catch { state.reset(); reporter.publish(unknown(version)) }
      }
      try {
        unsubscribe = context.data.listen(envelope => { if (!disposed) state.receive(envelope?.details) })
        if (typeof unsubscribe !== "function") throw new Error("unsupported subscription")
        timer = clock.setInterval(tick, 1000)
        tick()
      } catch {
        if (typeof unsubscribe === "function") { try { unsubscribe() } catch {} }
        void reporter.dispose(); return
      }
      return async () => {
        if (disposed) return
        disposed = true; clock.clearInterval(timer)
        try { unsubscribe() } catch {} // Observation failure must not interrupt the agent.
        state.reset()
        await reporter.dispose()
      }
    },
  }
}

export default createPlugin()
