# Handoff — 2026-09-27-copilot-cli-runner

**Last updated:** 2026-09-27T16:32:59Z
**Branch:** `feat/copilot-cli-runner`
**PR:** https://github.com/open-mercato/cezar/pull/1113 (draft)
**Current phase/step:** Phase 2 Step 2.3
**Last commit:** `75f013ae` — feat(core): add the Copilot ACP dialect

## What just happened

- Checkpoint 1 passed: full `npm run typecheck` and the full `npm test` (8296 tests) are green.
- The shared ACP layer (`acp-client.ts`, `acp-ui-mapper.ts`) is in, ported **verbatim** from draft
  PR #1049 so the eventual merge is an identical-file resolution. The only edit is two additive
  optional `AcpDialect` hooks (`toolStatusOf`, `parentItemOf`) that Copilot needs and Gemini does
  not; they are inert for a dialect that omits them.
- Spec Step 3.1 is settled against the real `@github/copilot` 1.0.88 and written up in
  `copilot-acp-notes.md`. It corrects the spec in three places: the invocation is `copilot --acp`
  (there is no `--stdio`), Copilot has a **native `plan` update channel**, and per-turn usage is a
  **top-level `usage` on the `session/prompt` result** — so `usage.updated` needs no substitute.
- `copilot` is now a real runner id everywhere the seam enumerates runners, and the dialect exists.

## Next concrete action

- Step 2.3 — write `packages/cezar/src/core/__fixtures__/copilot/` (`.ndjson` + `.expected.json`
  pairs) covering every `ui-parity.test.ts` capability row, plus a `README.md` citing
  `copilot-acp-notes.md` frame by frame.

## Blockers / open questions

- **No authenticated Copilot transcript.** The host `gh` token has no Copilot entitlement, so the
  fixtures cannot be captured the way #1049 captured Gemini's. They are derived instead from the
  CLI's own ACP emitters, read out of the bundle inside the installed binary — the same offline
  method the source spec used for codex. Every fixture header must say so, and Step 3.4's opt-in
  real-CLI smoke test is the live gate.
- `createRunner` currently **throws** for `copilot`. Step 3.1 replaces that with the real runner;
  do not ship without it.

## Environment caveats

- Dev runtime runnable: yes. `npm ci` done in this worktree.
- Browser / UI checks: skipped — no `.ai/qa/test-env.json` descriptor in the repo, and nothing
  user-drivable exists until the runner lands.
- Database/migration state: n/a.
- `npm test` needs `TMPDIR`, `TMP`, `TEMP` and `CEZ_*` cleared. `automations-gate.test.ts` is a
  known flake under concurrent-worktree load — re-run it in isolation before blaming the diff.
- A scratch Copilot CLI install used for verification lives at
  `/home/cezar/cezar/.ai/cezar/tmp/0da5390a-cd8b-4926-b58d-ac7792074f51/copilot-probe/`
  (`npm i @github/copilot@1.0.88` in a fresh dir recreates it).

## Worktree

- Path: `/home/cezar/cezar/.ai/cezar/worktrees/0da5390a-cd8b-4926-b58d-ac7792074f51`
- Created this run: no (reused the current linked worktree)
