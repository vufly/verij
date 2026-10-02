# Agy interactive callback feasibility probe

This is H0 evidence tooling, not the production Agy adapter. Installed runtime tested: **1.2.14**. [Retained results](interactive-results.json) prove startup callback execution and a controlled prior-command stdout composition case; they do not prove full work/input/background lifecycle semantics.

Run from the Verij worktree, explicitly supplying a readable authenticated profile token file:

```bash
python3 prototypes/agy-runtime/verify_interactive.py \
  --token-file "$AGY_PROBE_TOKEN" --scratch-dir ../artifacts/scratch \
  --output ../artifacts/agy-interactive-results.json
```

The harness copies that single token into a private mode-0700 scratch HOME with a mode-0600 token file. It writes a private `.gemini/antigravity-cli/settings.json`, starts fresh Agy in a sized controlling PTY, and answers terminal introspection queries without answering agent/permission prompts. It does not point writable scratch files back at the live profile. At completion it removes the copied credential and all private app-data/config/cache/state trees, preserving only callback metadata, sanitized configuration and the result. Raw terminal output and callback input are not persisted.

Two cases run: a direct [`statusline.py`](statusline.py) command rendering a controlled marker, and a wrapper forwarding original stdin to a controlled previous command and preserving its stdout, stderr and exit status. The tested previous command exits zero and emits `OLD_STATUS_MARKER`; nonzero failures, arbitrary user callbacks, disablement and concurrency are not claimed verified. Callback records contain field names and selected state/count/version values; identifiers are hashed and email, message, quota, path and token values are excluded.

Final scratch `vj-agy-3yp0h_li` records **12 real callbacks per case**, with authenticating → initializing and both markers actually rendered. Four distinct callback shapes per case are retained in the report; the full metadata NDJSON remains local. The wrapped case records zero exit and exact prior stdout preservation. No model prompt was sent. Idle did not appear within each 25-second observation window, so work, permissions, questions, background tasks, cancellation and terminal outcome remain pending.

Earlier Magy attempts set `ANTIGRAVITY_APP_DATA_DIR` without proving configuration load and captured no callbacks. The new positive control redirects HOME and supplies a controlling PTY with explicit dimensions; it establishes a working combination, not which earlier difference alone caused failure. Startup payloads omit optional tool/input/task fields, so absence must not be treated as a verified negative capability or inferred Done/Needs-input state. These passes do not approve H0.

## Interactive turns and signal limits — 2026-10-02

The harness now supports actual authenticated interactive turns, with a **private native tmux terminal** and a trust entry limited to the generated scratch workspace. First-run screens are driven only when recognized: welcome acknowledgement, unchecking the data-sharing checkbox, and confirming only after the native screen shows it unchecked and Done selected. These are test setup actions, not a monitoring adapter or a way to answer tool permissions. No global permission rule or `--dangerously-skip-permissions` is used.

```bash
python3 prototypes/agy-runtime/verify_interactive.py \
  --scenario no-tool --tmux --dismiss-welcome --decline-telemetry --timeout 35 \
  --token-file "$AGY_PROBE_TOKEN" --scratch-dir ../artifacts/scratch \
  --output ../artifacts/agy-turn-results.json
# Other independently bounded cases: permission, question, cancel.
python3 prototypes/agy-runtime/verify_observations.py
```

Workspace `.agents/hooks.json` installs only PreInvocation, PostInvocation, PostToolUse and Stop. [`lifecycle.py`](lifecycle.py) records metadata, emits `{}` for ordinary hooks and `{"decision":""}` for Stop. There is no PreToolUse permission gate, injected step or forced continuation. Native CLI PID/start identity is distinguished from Python callback PIDs. Native process termination is awaited before private auth/app-data cleanup; this fixes a detected late-write cleanup race.

| Case | Evidence | Result |
|---|---|---|
| No-tool turn | [turn-results.json](turn-results.json): Working → Idle; same-conversation PreInvocation → PostInvocation → Stop, fullyIdle=true, no error, NO_TOOL_CALL | PASS |
| Pending command approval | [permission-results.json](permission-results.json): Working → tool_use with tool_confirmation_pending=true; only PreInvocation, command not approved | PASS for pending signal, not approval/reply/full neutrality matrix |
| Native Alpha/Beta question | [question-results.json](question-results.json): test-only native choice widget present; callbacks remain Working without pending-input/tool-confirmation flag | LIMITATION: no observed structured question-pending signal |
| Ctrl+C after Working | [cancel-results.json](cancel-results.json): callback becomes Idle; only PreInvocation, no Stop during the five-second post-idle window | LIMITATION: Idle cannot establish successful completion |

`verify_observations.py` matches retained records to original scratch NDJSON, verifies neutral hook configuration and credential removal, and refuses to treat the two limitations as passes. Question UI recognition is a controlled test diagnostic only; **no terminal-text Needs-input heuristic is proposed**. This single question path does not prove that every possible runtime interface lacks a signal. Background `fullyIdle:false`, permission reply/resumption, actual terminal failure, arbitrary callback/nonzero-exit/disablement and concurrency remain pending.

Results are pre-teardown observation snapshots. Closing the native CLI appended two additional callbacks in each final run; the validator checks the corresponding original-record prefix, not full-file equality. Late shutdown callbacks remain retained and are not relabelled as turn completion or cancellation outcome evidence.

Eleven initial no-tool attempts and the first permission/question/cancel attempts remain in workspace artifacts. Most early runs were blocked by first-run UI; one backend turn completed while the UI callback still reported Initializing. Intermediate key-navigation probes did not verify persisted data-sharing state; an early boolean labelled “declined” was corrected to “attempted”. One question teardown detected private-tree recreation; its original FAIL is preserved and the tree was cleaned after process exit. [Capability summary](capabilities.json) records current supported observations and limitations. H0 remains unapproved.

## Permission resumption and aggregate background completion

Two additional **real authenticated 1.2.14 runs** pass:

- [permission-reply-results.json](permission-reply-results.json), scratch `vj-agy-1qlabzhn`: native pending confirmation → one-time reply → PostToolUse without error → continued model invocation → Stop fullyIdle=true → Idle.
- [background-results.json](background-results.json), scratch `vj-agy-6r0fyloe`: one eight-second asynchronous command produces `task_count=1`. The model becomes **Idle with task_count=1** and emits **Stop fullyIdle=false**. After the command finishes, PostToolUse and another model invocation precede a later **Stop fullyIdle=true**. Both Stop events use `executionNum=0`, so that number is not a unique monotonic completion identity.

```bash
python3 prototypes/agy-runtime/verify_interactive.py \
  --scenario permission-reply --tmux --dismiss-welcome --decline-telemetry \
  --token-file "$AGY_PROBE_TOKEN" --scratch-dir ../artifacts/scratch \
  --output ../artifacts/agy-permission-reply-results.json
python3 prototypes/agy-runtime/verify_interactive.py \
  --scenario background --tmux --dismiss-welcome --decline-telemetry \
  --token-file "$AGY_PROBE_TOKEN" --scratch-dir ../artifacts/scratch \
  --output ../artifacts/agy-background-results.json
```

The test driver verifies both the exact native command summary/body and the numbered **one-time** option. It selects only `printf VJ_PERMISSION` or `sleep 8; printf VJ_BACKGROUND`, never the conversation-wide/persistent Always Allow choices. Unexpected commands, extra body rows and unrecognized controls stay blocked. This is an operator action in a generated probe, **not a permission hook, application approval flow or monitoring heuristic**. Five modal boundary tests cover the exact match, fixed `Run this command?` label, added commands, unexpected summaries and Always Allow refusal. Earlier blocked scripts used an incorrect option label/overstrict static-row check; original attempts remain in workspace.

### Rejected neutral-question hook experiment

An explicit opt-in `--pretool-observer` installs only a narrowly matched run_command/ask_question handler returning **empty JSON**, not allow/deny/ask or overrides. However [pretool-rejection-results.json](pretool-rejection-results.json), `vj-agy-82ede5kk`, does **not** reproduce the default pending-permission behavior: it records the exact tool attempt, then continuation/Stop without pending confirmation or PostToolUse. A corresponding question attempt (`artifacts/agy-pretool-question-attempt-1.json`, `vj-agy-pzy_971y`) likewise records ask_question but no native choice UI. The exact internal cause is not established; empty output is **not accepted as neutral** and is excluded from approved-command/background probes. A tool-attempt event alone does not establish a real pending question.

### Headless deadline counterexample

[deadline-results.json](deadline-results.json), `vj-agy-z2gfwc1n`, requested `--print-timeout 2s`. It emitted final stream `SUCCESS`, exit 0 and an empty response, while lifecycle recorded only PreInvocation—no PostInvocation/Stop. This is a **LIMITATION**, not a successfully completed model turn or the intended genuine-terminal-error proof. The same result occurred in two earlier attempts. Caller-imposed deadlines must remain part of outcome context; do not derive completed work solely from wrapper exit/final SUCCESS in this path.

Startup status-line callbacks **also executed in this headless test**, contradicting a universal “headless never invokes statusLine” claim for 1.2.14. They remained Authenticating/Initializing, so this does not establish a complete headless work-status adapter. Historical no-callback tests lacked a positive config control and remain qualified. True fatal execution errors, neutral question signals, broader background/cancellation modes and production integration remain pending.
