# Open-issue audit and dispatched fixes

## Goal
Audit open GitHub reports and deliver five independently tested fix PRs through Codex gpt-5.6-luna children. Root only orchestrates and verifies.

## Scope and ranking
Snapshot: 114 open issues, 51 open PRs, 2026-09-28. Exclude covered fixes and active claims. Existing serious fixes include #913/#1003 and #882/#888. Choose uncovered, actionable regressions with operational impact.
1. #1007: installed-service port drift risks forwarding to the wrong process. Dispatch after #1000 to avoid startup-file overlap.
2. #1000: wrong injected API origin breaks dispatch/reporting; reproduced in this task (localhost unreachable, host gateway owns this tree).
3. #1079: finalize autosave includes check-generated tracked artifacts.
4. #1077: editing automations can replay label events and launch duplicate work.
5. #1059: live cache updates erase user-owned task titles.

## Non-goals
No base-branch merges, release, deployment, unrelated feature implementation, duplicate PRs, or shared-state repairs. #1078 needs a product decision about queued snapshots and is deferred behind proven regressions. Root creates no umbrella code PR: user explicitly assigns PR creation to children.

## Implementation Plan
### Phase 1: Independent PRs
1.1 Dispatch #1000 (startup/API ownership), #1079 (workflow autosave), #1077 (automation receipts), #1059 (web title caches) in disjoint scopes.
1.2 Validate reports and dispatch #1007 once #1000 settles; require reviewed child PRs, regression proof and configured validation.
### Phase 2: Verification
2.1 Inspect every child diff and rerun reported targeted tests, retain separate PRs.
2.2 Dispatch exactly one final review with --review-of cez/df4dcc59 to inspect this audit plus all five PR heads and validation evidence; await verdict. No child integration needed: separate PRs are the requested outputs.

## Risks
Owning cockpit must be reached at http://172.17.0.1:4321 in this container. Use CEZ_API_URL override only for task commands. Children must check current main/PRs and claims before fixing; stale reports may already be resolved. At most eight total children, four in flight. Six planned (five implementers + final review), two reserved for recovery. gpt-5.6-luna confirmed in live Codex catalog.

## Progress

> Convention: `- [ ]` pending, `- [x]` done. Append ` — <commit sha>` when a step lands. Do not rename step titles.

### Phase 1: Independent PRs

- [ ] 1.1 Dispatch first four issue fixes
- [ ] 1.2 Validate first reports and dispatch service-port fix

### Phase 2: Verification

- [ ] 2.1 Verify child diffs and test evidence
- [ ] 2.2 Obtain final independent review verdict
