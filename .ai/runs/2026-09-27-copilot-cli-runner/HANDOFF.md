# Handoff — 2026-09-27-copilot-cli-runner

**Last updated:** 2026-09-27T00:00:00Z
**Branch:** `feat/copilot-cli-runner`
**PR:** not yet opened
**Current phase/step:** Phase 1 Step 1.1
**Last commit:** — (run folder is the first commit)

## What just happened
- Routed from `om-auto-create-pr` to the loop engine (26 Steps > threshold 20).
- Settled spec Step 3.1 against the real `@github/copilot` 1.0.88 binary before planning, which
  corrected the spec's `copilot --acp --stdio` to `copilot --acp` and confirmed the handshake,
  the auth-error frame, the token precedence and the tool-filter flags (PLAN.md § Verified).

## Next concrete action
- Step 1.1 — port `packages/cezar/src/core/acp-client.ts` (+ test) verbatim from `pr-1049`.

## Blockers / open questions
- No authenticated Copilot transcript is available on this host (the `gh` token has no Copilot
  entitlement), so fixtures are derived from the ACP schema and the installed binary rather than
  a recorded live session. Documented in PLAN.md § Risks; Step 3.4 is the live gate.

## Environment caveats
- Dev runtime runnable: yes (`npm ci` completed in this worktree).
- Browser / UI checks: not yet attempted; no `.ai/qa/test-env.json` descriptor in the repo.
- Database/migration state: n/a — cezar keeps plain JSON/NDJSON state, no database.
- `npm test` needs `TMPDIR`/`TMP`/`TEMP` and `CEZ_*` cleared, or ~10 unrelated tests fail.

## Worktree
- Path: `/home/cezar/cezar/.ai/cezar/worktrees/0da5390a-cd8b-4926-b58d-ac7792074f51`
- Created this run: no (reused the current linked worktree)
