// Synthetic API fixture shared by scheduling and real Zellij/native-store probes.
export function fixture(memberCount = 1) {
  const calls = Object.fromEntries(["session.status", "session.get", "session.children",
    "session.messages", "permission.list", "question.list"].map(name => [name, 0]))
  const listeners = new Map()
  const root = { id: "root", title: "Synthetic resource probe" }
  const children = Array.from({ length: memberCount - 1 }, (_, i) => ({ id: `child-${i}`, parentID: root.id }))
  let activity = "idle", turn = 1, completed = 0
  const response = (endpoint, value) => { calls[endpoint]++; return Promise.resolve({ data: value }) }
  const api = {
    app: { version: "1.18.34" },
    route: { current: { name: "session", params: { sessionID: root.id } } },
    state: { ready: true },
    client: {
      session: {
        status: () => response("session.status", { root: { type: activity } }),
        get: () => response("session.get", root),
        children: ({ sessionID }) => response("session.children", sessionID === root.id ? children : []),
        messages: () => response("session.messages", completed ? [
          { info: { id: `user-${turn}`, role: "user" } },
          { info: { id: `assistant-${turn}`, role: "assistant", parentID: `user-${turn}`,
            finish: "stop", time: { completed } } },
        ] : activity === "idle" ? [] : [{ info: { id: `user-${turn}`, role: "user" } }]),
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
