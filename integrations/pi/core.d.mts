import type { ExtensionAPI } from "@earendil-works/pi-coding-agent"

export const PROTOCOL_VERSION: number
export const MINIMUM_VERSION: string
export const SUPPORTED_VERSIONS: string

export const ERROR_KINDS: readonly string[]
export const REQUEST_KINDS: readonly string[]
export const PI_ACTIVITIES: readonly string[]
export const ALLOWED_STATUSES: readonly string[]
export const ALLOWED_READY_KEYS: readonly string[]
export const ALLOWED_RECEIPT_KEYS: readonly string[]
export const ALLOWED_SNAPSHOT_KEYS: readonly string[]

export const observationBudget: {
  readonly leaseMs: number
  readonly maxBackoffMs: number
  readonly startupTimeoutMs: number
  readonly receiptTimeoutMs: number
  readonly shutdownWaitMs: number
  readonly maxQueueSize: number
  readonly minWriteIntervalMs: number
}

export function supportsVersion(version: unknown): boolean
export function isValidU32(val: unknown): boolean
export function isControlChar(code: number): boolean
export function hasLoneSurrogate(str: unknown): boolean
export function sanitizeId(id: unknown): string | undefined
export function sanitizeTitle(title: unknown): string | undefined
export function classifyError(error: unknown): string
export function mapPromptKind(kind: unknown): string
export function unknown(version?: string): Record<string, unknown>
export function validateStrictSnapshot(snap: unknown, expectedConvId?: string | null): boolean
export function snapshotsSemanticallyEqual(a: unknown, b: unknown): boolean

export class SnapshotQueue {
  constructor(limit?: number, onOverflow?: (() => void) | null)
  limit: number
  items: string[]
  overflow: boolean
  inFlight?: string
  delivered?: string
  totalCount(): number
  push(value: unknown, renew?: boolean): void
  shift(): string | undefined
  acknowledge(): void
  requeueInFlight(): void
  reset(): void
}

export class StateProjector {
  constructor(options?: {
    version?: string
    randomUUID?: () => string
    runtimeId?: string
    conversationId?: string
    conversationTitle?: string
  })
  version: string
  runtimeId: string
  conversationId?: string
  conversationTitle?: string
  aggregateRunId: string | null
  activity: string | null
  previousExecutionActivity: string | null
  pendingRequests: Array<{ id: string; kind: string }>
  backgroundTaskCount: number
  candidateOutcome: unknown
  outcome: unknown
  activePrompt: { id: string; kind: string } | null
  promptSavedState: unknown
  runActive: boolean
  genuineAggregate: boolean
  compactionActive: boolean
  activeToolCount: number
  degradedToUnknown: boolean
  invalidateOnOverflow(): void
  handleEvent(type: string, event?: Record<string, unknown>, ctx?: Record<string, unknown>): Record<string, unknown>
  getSnapshot(): Record<string, unknown>
}

export interface BridgeOptions {
  pi?: ExtensionAPI
  version?: string
  pane?: string | number
  session?: string | null
  pid?: number
  reporter?: string
  runtimeDir?: string
  statesDir?: string
  conversationId?: string
  conversationTitle?: string
}

export interface BridgeInstance {
  install: (target?: unknown) => void
  handleEvent: (type: string, event?: unknown, ctx?: unknown) => void
  startReporter: () => void
  publish: (snapshot: Record<string, unknown>, force?: boolean) => void
  teardown: () => void
  getProjector: () => StateProjector
  getQueue: () => SnapshotQueue
  isConnected: () => boolean
  isStopped: () => boolean
}

export function createBridge(options?: BridgeOptions, runtime?: Record<string, unknown>): BridgeInstance
