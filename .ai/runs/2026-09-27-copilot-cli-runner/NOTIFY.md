# Notify — 2026-09-27-copilot-cli-runner

> Append-only log. Every entry is UTC-timestamped. Never rewrite prior entries.

## 2026-09-27T15:54:57Z — run started
- Brief: implement issue #582 — GitHub Copilot CLI as a first-class cezar runner over ACP
  (source spec `.ai/specs/2026-09-19-runner-seam-native-backends.md`, Phase 3 only).
- External skill URLs: none.

## 2026-09-27T15:54:57Z — decision: engine routed to the loop
- `om-auto-create-pr` drafted 26 Steps against `engine.loopStepThreshold` 20, so the run was
  handed to `om-auto-create-pr-loop` before anything was written.

## 2026-09-27T15:54:57Z — decision: this run carries the shared ACP layer
- Spec Phase 0 has not landed and spec Phase 2 (`gemini`, #581) is open **draft** PR #1049, so
  per the spec's § Phasing rule Phase 3 carries Steps 2.2–2.3.
- To avoid forking a second ACP client, `core/acp-client.ts` and `core/acp-ui-mapper.ts` are
  taken from PR #1049 **verbatim**; whichever PR lands second drops its copy.

## 2026-09-27T15:54:57Z — decision: spec Step 3.1 settled before planning
- Verified against the real `@github/copilot` 1.0.88 binary. The spec's `copilot --acp --stdio`
  is wrong — there is no `--stdio` flag; the invocation is `copilot --acp`.
- Confirmed the `initialize` capabilities, the `-32000 Authentication required` frame, the
  `COPILOT_GITHUB_TOKEN` > `GH_TOKEN` > `GITHUB_TOKEN` precedence and the tool-filter flags.

## 2026-09-27T15:54:57Z — blocker (non-fatal): no authenticated Copilot transcript
- The host `gh` token carries no Copilot entitlement, so `session/new` answers
  "Authentication required" and no live streaming frames could be recorded. Fixtures are derived
  from the ACP schema (`PROTOCOL_VERSION = 1`) and from strings in the installed
  `@github/copilot-linux-x64` binary, each cited in its fixture header — the same offline method
  the source spec used for codex. Step 3.4's opt-in smoke test is the live gate.
