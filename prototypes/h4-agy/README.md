# H4 Agy/Magy production verification

This fixture executes the production CLI, installed callback asset and stock
Zellij exporter. It copies an authenticated token into a private scratch home,
pins a private copy of the installed Agy binary, and invokes real no-tool/model
turns plus two exact harmless command probes. It records whitelisted callback
metadata and original production state snapshots, not raw terminal transcripts.

```bash
make dev
python3 prototypes/h4-agy/verify.py \
  --magy-python /absolute/path/to/magy-environment/bin/python \
  --token-file "$AGY_PROBE_TOKEN" \
  --scratch-dir ../artifacts/scratch \
  --output ../artifacts/h4-results.json
python3 prototypes/h4-agy/verify_evidence.py ../artifacts/h4-results.json
```

The selected Python interpreter must import the installed Magy package. The
fixture uses its actual pane/watch/run APIs from a real launcher shell. Its
generated Git repository and private Magy registry belong to scratch; no user
profile, permissions, repository or session is changed. Only the exact native
one-time confirmation for `printf VJ_H4_PERMISSION` and
`sleep 8; printf VJ_H4_BACKGROUND` is selected. No blanket auto-approval or
`PreToolUse` observer is installed.

Covered: idempotent setup/doctor, startup presence and process/pane identity,
Working, Stop-qualified success/revision idempotence, previous visible stdout and
neutral hooks, native permission/reply, background-task deferral/aggregate Stop,
session rename, process exit, profile-home pane launch, real watch stream and
runner binding, observer restart, completed/cancelled watch auto-close, detached
run exclusion, and uninstall preservation. Cleanup validates recorded process
births before terminating any remainder, removes private authentication/profile
homes and Magy request/log storage, and closes private socket/terminal contexts.

Automated terminal failure/log-rotation/out-of-order tests have their own scope.
Full two-host navigation/acknowledgement, question and cancellation semantics,
custom-mux live execution, and human UX approval remain separate. Review using
[the setup/capability guide](../../integrations/agy/README.md); record **approve
H4** or **rework H4** with concrete pane/scenario feedback before H5.

The user accepted the current integrated implementation on 2026-10-11 and
requested commit/push, master integration and global build. This is recorded
separately in [approval.json](approval.json); original automated evidence retains
its unapproved-at-run-time flag. The full H5 release matrix remains separate.
