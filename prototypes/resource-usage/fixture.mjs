// Synthetic API fixture shared by scheduling and real Zellij/native-store probes.
export function fixture(memberCount = 1) {
  const calls = Object.fromEntries(["session.status", "session.get", "session.children",
    "session.messages", "permission.list", "question.list"].map(name => [name, 0]))
  const listeners = new Map()
  const root = { id: "root", title: "Synthetic resource probe" }
  const children = Array.from({ length: memberCount - 1 }, (_, i) => ({ id: `child-${i}`, parentID: root.id }))
  let activity = "idle", turn = 1, completed = 0
  const response = (endpoint, value, cursor) => {
    calls[endpoint]++
    return Promise.resolve({ data: value, response: { headers: new Headers(cursor ? { "x-next-cursor": cursor } : {}) } })
  }
  const messages = () => completed ? [
    { info: { id: `user-${turn}`, sessionID: "root", role: "user", time: { created: turn * 10 } } },
    { info: { id: `assistant-${turn}`, sessionID: "root", role: "assistant", parentID: `user-${turn}`,
      finish: "stop", time: { created: turn * 10 + 1, completed } } },
  ] : activity === "idle" ? [] : [{ info: { id: `user-${turn}`, sessionID: "root", role: "user", time: { created: turn * 10 } } }]
  const api = {
    app: { version: "1.18.34" },
    route: { current: { name: "session", params: { sessionID: root.id } } },
    state: { ready: true },
    client: {
      session: {
        status: () => response("session.status", { root: { type: activity } }),
        get: () => response("session.get", root),
        children: ({ sessionID }) => response("session.children", sessionID === root.id ? children : []),
        messages: ({ limit = 100, before } = {}) => {
          const all = messages(), end = before ? Number(before) : all.length
          const start = Math.max(0, end - limit)
          return response("session.messages", all.slice(start, end), start ? String(start) : undefined)
        },
      },
      permission: { list: () => response("permission.list", []) },
      question: { list: () => response("question.list", []) },
    },
    event: { on: (type, listener) => { listeners.set(type, listener); return () => listeners.delete(type) } },
    lifecycle: { onDispose: dispose => { api.dispose = dispose } },
  }
  return { api, listeners, calls, count: () => Object.values(calls).reduce((a, b) => a + b, 0),
    activity: value => { activity = value; completed = 0 }, finish: () => { activity = "idle"; completed = Date.now() },
    newTurn: () => ++turn }
}
