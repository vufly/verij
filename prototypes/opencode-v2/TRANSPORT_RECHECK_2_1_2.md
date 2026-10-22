# Transport blocker recheck — OpenCode 2.1.2

Date: **2026-10-21 UTC**. Result: **the source-level transport blocker remains**.
P0 and production admission remain blocked. The qualified target has not been
changed; all retained 2.0.32 runtime evidence remains scoped to that version.

## Source pin and method

The installed executable reports `opencode v2.1.2`. The upstream `v2.1.2` tag
resolves to commit `a517687fde98a7329ada3ae644a01a9fc6c4e99e`.
Seven files were downloaded from that immutable commit into a disposable
`/tmp` directory. No upstream code was executed, and no OpenCode client/server
or global installation was started or changed during this recheck.

The existing `transport_source_audit.characterize` checks passed all eight
boundary facts on these sources. This is a supplementary source recheck, not
the original hash-pinned audit report or runtime transport qualification.
Six sources are byte-identical to the retained 2.0.32 audit; the generated
client changed, but its relevant request/constructor/JSON parsing boundaries
did not.

| Source path | SHA-256 at 2.1.2 |
|---|---|
| `packages/plugin/src/tui/context.ts` | `db984caa696e830185dfd99493e94acbda3aa4ced50b19c3aafdaeeb4db442fa` |
| `packages/tui/src/plugin/api.tsx` | `8e836be5f9178a6759dd36d9c68475a02e844ea039e6d1595d73f622bd63dbe5` |
| `packages/tui/src/context/client.tsx` | `714c7f71f6073d27a1cfd52a421f3940a476eb4749e343925507ab5d8c4a4ea6` |
| `packages/tui/src/context/data.tsx` | `e1722ee1982cbd85623756778a424ccd323852f29cc4667d63cf7aa00e47a6ad` |
| `packages/client/src/promise/generated/client.ts` | `ce01b3473b441552343529563659db82f785e995ef4771912934d5fb5e110043` |
| `packages/cli/src/services/server-connection.ts` | `9ab1863048e60f427d5e27daff8536fb888b8c261d2e89d9bfb937f60b785dd7` |
| `packages/cli/src/services/standalone.ts` | `dc8197da471d3d213acf51ef809bb1c05a7cf697bce1f0795e49fdd4c0165cf9` |

Source URLs use this prefix followed by the table path:
`https://raw.githubusercontent.com/anomalyco/opencode/a517687fde98a7329ada3ae644a01a9fc6c4e99e/`.

## Findings

- The public context still exposes `client`, not an observation factory or
  connection-generation contract. Plugin construction still assigns
  `client: host.client.api`.
- Request options remain `signal`, `headers`, and `onActivity`. The SDK captures
  constructor-owned `fetch`; success and declared-error paths still call the
  full-body JSON helper, which reads `response.text()` then `JSON.parse`.
- Standalone still generates its random credential outside the public plugin
  context. Explicit-server authentication also resolves outside that context.
- The host can replace its API on reconnect; the data cache uses the host
  connection and dynamic API getter. These remain source observations, not a
  newly reproduced reconnect failure.

Therefore an ordinary client call still cannot enforce Verij's byte/depth/node
bounds before SDK parsing. The installed version being newer does not unblock
P0. No 2.1.2 runtime, identity, family-freshness, or resource qualification is
claimed.

## Recommended next actions

1. **Await upstream design feedback.** Feature request
   [anomalyco/opencode#57885](https://github.com/anomalyco/opencode/issues/57885)
   was filed with user approval on 2026-10-21. Use
   [the existing transport requirement](UPSTREAM_TRANSPORT_REQUIREMENT.md) as
   its specification: an independently owned read-only observation client,
   transport decoration before parsing, actual authenticated connection
   provenance, replacement/generation notification, and independent disposal.
   The issue includes the oversized-success/error characterization and
   distinguishes 2.0.32 synthetic tests from the 2.1.2 source-only recheck.
   Wait for maintainer design feedback before an implementation PR.
2. **For faster local progress, approve a separate patched-target experiment.**
   Add the capability in a disposable upstream checkout, pin its source and
   binary hashes, and test it only through the private harness. This would
   require an explicit scope decision; it must not replace the installed
   executable, modify the shared service, or count as stock OpenCode support.
3. **Once a real hook exists, qualify transport first.** Exercise standalone,
   authenticated explicit-server, and path-prefix modes, including oversized
   bodies, deadlines, physical cancellation, reconnect fencing, and cleanup.
   Then finish terminal identity and complete-family/input freshness before
   reconsidering P0. A transport patch alone does not pass the gate.
4. **If neither dependency path is wanted, pause V2 admission.** Keep V1
   behavior intact and report V2 monitoring as unsupported. Do not substitute
   event-only completion, guessed credentials, shared-fetch mutation, or
   post-parse projection for the approved contract.
