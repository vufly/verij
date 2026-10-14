// Pi 1.2.0 monitoring extension entrypoint for Verij.
// Metadata-only aggregate projection with bounded transport.
import { closeSync, constants, fstatSync, openSync, readSync } from "node:fs"
import { homedir } from "node:os"
import { join } from "node:path"
import {
  VERSION,
  getAgentDir,
  type AgentBeforeSettleEvent,
  type AgentEndEvent,
  type AgentSettledEvent,
  type AgentStartEvent,
  type BeforeAgentStartEvent,
  type ExtensionAPI,
  type ExtensionContext,
  type SessionBeforeCompactEvent,
  type SessionCompactEvent,
  type SessionCompactFailedEvent,
  type SessionInfoChangedEvent,
  type SessionShutdownEvent,
  type SessionStartEvent,
  type SessionTreeEvent,
  type ToolExecutionEndEvent,
  type ToolExecutionStartEvent,
  type TurnEndEvent,
  type TurnStartEvent,
  type UIPromptEndEvent,
  type UIPromptStartEvent,
} from "@earendil-works/pi-coding-agent"
import {
  createBridge,
  hasLoneSurrogate,
  isControlChar,
  isValidU32,
  supportsVersion,
  type BridgeInstance,
} from "./core.mjs"

interface AdapterConfig {
  reporter?: string
  runtime_dir?: string
  states_dir?: string
}

function isValidConfigPath(val: unknown): val is string {
  if (typeof val !== "string" || val.length === 0 || val.length > 4096) return false
  if (val.startsWith("~")) return false // Explicit path required; no tilde guessing
  if (hasLoneSurrogate(val)) return false
  for (let i = 0; i < val.length; i++) {
    if (isControlChar(val.charCodeAt(i))) return false
  }
  return true
}

export function resolveAgentDir(): string {
  if (typeof getAgentDir === "function") {
    try {
      return getAgentDir()
    } catch {}
  }
  const envDir = process.env.PI_CODING_AGENT_DIR
  if (envDir) {
    if (envDir === "~") {
      return homedir()
    }
    if (envDir.startsWith("~/")) {
      return join(homedir(), envDir.slice(2))
    }
    return envDir
  }
  return join(homedir(), ".pi", "agent")
}

function loadSiblingAdapterConfig(): AdapterConfig | null {
  let fd: number | null = null
  try {
    const agentDir = typeof getAgentDir === "function" ? getAgentDir() : resolveAgentDir()
    const configPath = join(agentDir, "verij-monitoring", "adapter.json")
    fd = openSync(configPath, constants.O_RDONLY | constants.O_NONBLOCK)
    const stat = fstatSync(fd)
    if (!stat.isFile() || stat.size > 16384) {
      return null
    }

    const buf = Buffer.alloc(16385)
    const bytesRead = readSync(fd, buf, 0, 16385, 0)
    if (bytesRead > 16384) {
      return null
    }

    const decoder = new TextDecoder("utf-8", { fatal: true })
    const content = decoder.decode(buf.subarray(0, bytesRead))
    const parsed = JSON.parse(content)
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return null

    const config: AdapterConfig = {}
    if (isValidConfigPath(parsed.reporter)) config.reporter = parsed.reporter
    if (isValidConfigPath(parsed.runtime_dir)) config.runtime_dir = parsed.runtime_dir
    if (isValidConfigPath(parsed.states_dir)) config.states_dir = parsed.states_dir
    return config
  } catch {
    return null
  } finally {
    if (fd !== null) {
      try {
        closeSync(fd)
      } catch {}
    }
  }
}

export default function (pi: ExtensionAPI): void {
  // Read host version only; reject unsupported versions
  if (!supportsVersion(VERSION)) {
    return
  }

  let bridge: BridgeInstance | null = null

  pi.on("session_start", (event: SessionStartEvent, ctx: ExtensionContext) => {
    // Only bind in interactive TUI mode; zero file I/O in non-TUI contexts
    if (ctx.mode !== "tui") return

    const pane = process.env.ZELLIJ_PANE_ID
    if (!pane || !isValidU32(pane)) return

    const pid = process.pid
    if (!isValidU32(pid)) return

    const session = process.env.ZELLIJ_SESSION_NAME ?? null

    // Sibling adapter config loaded only on session_start in TUI mode (bounded read)
    const adapterConfig = loadSiblingAdapterConfig()
    const reporter = process.env.VERIJ_BIN || adapterConfig?.reporter || "verij"
    const runtimeDir = process.env.VERIJ_AGENT_STATE_DIR || adapterConfig?.runtime_dir
    const statesDir = process.env.VERIJ_STATES_DIR || adapterConfig?.states_dir

    if (!bridge) {
      bridge = createBridge({
        pi,
        version: VERSION,
        pane,
        session,
        pid,
        reporter,
        runtimeDir,
        statesDir,
      })
    }

    bridge.handleEvent("session_start", event, ctx)
  })

  pi.on("session_shutdown", (event: SessionShutdownEvent, ctx: ExtensionContext) => {
    if (bridge) {
      bridge.handleEvent("session_shutdown", event, ctx)
      bridge = null
    }
  })

  pi.on("session_info_changed", (event: SessionInfoChangedEvent, ctx: ExtensionContext) => {
    bridge?.handleEvent("session_info_changed", event, ctx)
  })

  pi.on("session_tree", (event: SessionTreeEvent, ctx: ExtensionContext) => {
    bridge?.handleEvent("session_tree", event, ctx)
  })

  pi.on("session_before_compact", (event: SessionBeforeCompactEvent, ctx: ExtensionContext) => {
    bridge?.handleEvent("session_before_compact", event, ctx)
  })

  pi.on("session_compact", (event: SessionCompactEvent, ctx: ExtensionContext) => {
    bridge?.handleEvent("session_compact", event, ctx)
  })

  pi.on("session_compact_failed", (event: SessionCompactFailedEvent, ctx: ExtensionContext) => {
    bridge?.handleEvent("session_compact_failed", event, ctx)
  })

  pi.on("before_agent_start", (event: BeforeAgentStartEvent, ctx: ExtensionContext) => {
    bridge?.handleEvent("before_agent_start", event, ctx)
  })

  pi.on("agent_start", (event: AgentStartEvent, ctx: ExtensionContext) => {
    bridge?.handleEvent("agent_start", event, ctx)
  })

  pi.on("agent_end", (event: AgentEndEvent, ctx: ExtensionContext) => {
    bridge?.handleEvent("agent_end", event, ctx)
  })

  pi.on("agent_before_settle", (event: AgentBeforeSettleEvent, ctx: ExtensionContext) => {
    bridge?.handleEvent("agent_before_settle", event, ctx)
  })

  pi.on("agent_settled", (event: AgentSettledEvent, ctx: ExtensionContext) => {
    bridge?.handleEvent("agent_settled", event, ctx)
  })

  pi.on("turn_start", (event: TurnStartEvent, ctx: ExtensionContext) => {
    bridge?.handleEvent("turn_start", event, ctx)
  })

  pi.on("turn_end", (event: TurnEndEvent, ctx: ExtensionContext) => {
    bridge?.handleEvent("turn_end", event, ctx)
  })

  pi.on("tool_execution_start", (event: ToolExecutionStartEvent, ctx: ExtensionContext) => {
    bridge?.handleEvent("tool_execution_start", event, ctx)
  })

  pi.on("tool_execution_end", (event: ToolExecutionEndEvent, ctx: ExtensionContext) => {
    bridge?.handleEvent("tool_execution_end", event, ctx)
  })

  pi.on("ui_prompt_start", (event: UIPromptStartEvent, ctx: ExtensionContext) => {
    bridge?.handleEvent("ui_prompt_start", event, ctx)
  })

  pi.on("ui_prompt_end", (event: UIPromptEndEvent, ctx: ExtensionContext) => {
    bridge?.handleEvent("ui_prompt_end", event, ctx)
  })
}
