# Required upstream connection/transport contract

**Dependency proposal, not an available API.** Do not implement calls to the
illustrative factory below against 2.0.32. The tagged public context does not
provide it, and current evidence does not qualify production V2 monitoring.

Upstream feature request: [anomalyco/opencode#57885](https://github.com/anomalyco/opencode/issues/57885), filed with user approval on 2026-10-21. Await maintainer design feedback before an implementation PR; filing does not unblock P0.

## Problem demonstrated

- The tagged Promise client parses complete success and declared-error bodies
  before returning. `sdk-results.json` reproduces missing byte/depth/node
  guards with an independently owned synthetic transport.
- The public CLI context exposes a connected client, but not its constructor
  transport or authenticated connection provenance.
- The prototype bounded GET transport works with an explicitly supplied private
  server endpoint/password. That does not establish how a production plugin
  obtains the actual standalone or remote client's connection safely.

Discovering defaults, reading credentials from service files/process state,
patching shared client methods/global fetch, or proxying normal execution are
not acceptable solutions. Keep standalone admission blocked without a supported
connection path.

## Pinned source audit and exit decision

The [offline source audit](transport-source-results.json) pins seven complete
2.0.32 files and checks the public context, constructed API projection, SDK
constructor/request/parser paths, standalone credential creation, explicit
server auth resolution, and host reconnect/data-cache boundaries. Its success
status is source characterization only; `p0_status` remains `BLOCKED`.

The host can replace its API on reconnect, but plugin construction captures
the current `host.client.api`. This source-level lifetime concern strengthens
the need for a supported connection-generation contract; it is not a runtime
reconnect failure claim. [The exit decision](P0_GATE_DECISION.md) preserves all
remaining qualification work and explains why a harness-only factory cannot
unblock the installed target.

## Minimum supported hook

Upstream should provide a way to construct an **independent observation client**
using the CLI's actual current endpoint/auth/transport. Its implementation may
keep endpoint credentials opaque. For example, a future factory could accept
an adapter-owned fetch decorator and return a read-only clone together with an
opaque connection-generation token. This is a suggested capability, not a
specific API name or signature to assume.

Required semantics:

1. Apply the decorator to the resolved authenticated transport **before** any
   success/error JSON parsing or full body buffering owned by the SDK.
2. Preserve the real endpoint's path prefix, authentication, TLS/custom fetch
   configuration, and standalone/private connection provenance. Do not use an
   unrelated default server or expose secrets in options/manifests.
3. Do not replace or mutate the agent's shared client/fetch. Disposing the
   observation clone must not disrupt normal OpenCode execution.
4. Expose replacement/disconnection information and an opaque generation so
   route/producer/connection replacement can revoke stale observations and
   queued work. An old in-flight read must never become current authority.
5. Preserve caller cancellation. The adapter must account for actual physical
   requests until body/cancellation teardown settles, independently of early
   caller deadlines. Implicit pagination/retries must not bypass accounting.
6. Permit GET-only enforcement, redirect rejection, and pre-parse byte,
   UTF-8, depth, and structural-node guards on every observation/error path.

A narrower, appropriately bounded metadata API may be an alternative if its
identity, input, family freshness, error, and authentication contracts are
separately proven. A small response under ordinary conditions is not a bound.

## Acceptance before unblocking

Use pinned 2.0.32 or a newly pinned release that actually ships the supported
hook. Qualify the actual interactive CLI in standalone and explicit-server
modes, plus an authenticated private remote-style endpoint with a URL prefix.

Exercise oversized user/tool/error bodies, misleading/absent Content-Length,
stalled/chunked reads, invalid UTF-8, deep/compact JSON, redirects, cancellation
that ignores abort, disconnect/replacement, and cleanup. Retain the existing
64 KiB outcome / 1 MiB general ceilings, depth 32, 16,384 structural nodes,
three-second collection deadline, four physical/four pending requests,
32-invocation/second rate with eight-call burst, and zero shared-client mutation.

Compare the observation client's connection generation with selected CLI
ownership throughout recovery. A passing headless/private-credential fixture
or a passing fake-fetch suite does not satisfy this acceptance gate.
