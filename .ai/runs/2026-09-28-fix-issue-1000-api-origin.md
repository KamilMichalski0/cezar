# Fix issue #1000: preserve cockpit API ownership

## Goal

Ensure dispatched agents inherit an API URL reachable on the interface where the cockpit listens, and ensure automatic port selection checks that same interface. Add focused regression coverage without changing dispatch protocol or workflow behavior.

## Scope

- `packages/cezar/src/index.ts` startup URL construction and port probe.
- A small testable helper module and unit tests for host-aware API URL and port probing.

## Non-goals

- No changes to `workflows/run.ts`, automation behavior, project registry, or server-install configuration.
- No instance discovery, health identity protocol, or orphan-process lifecycle redesign.

## Implementation Plan

### Phase 1: Reproduce and implement

- [x] 1.1 Extract/test host-aware API URL formatting and port-probe host selection. — 0b5dc705
- [x] 1.2 Wire the helpers into cockpit startup and preserve loopback defaults. — 0b5dc705

### Phase 2: Validate and publish

- [ ] 2.1 Run red-before-fix and green regression tests, then the configured validation gate.
- [ ] 2.2 Run the authoritative PR review/autofix pass and finalize the PR.

### Evidence

- Red regression: reversing the implementation while retaining `api-origin.test.ts` failed with `Cannot find module './api-origin.ts'` (exit 1); the committed implementation then passed both tests.
- Green regression: `npm exec vitest run packages/cezar/src/api-origin.test.ts` — 2 passed.
- Configured gate attempted in order. `npm run typecheck`, `npm test`, `npm run test:unit`, `npm run build`, and `npm run test:package` are blocked by pre-existing dependency/generated-artifact drift (Zod v3 resolving against v4-only contract APIs, missing `dotenv`/`proper-lockfile`, unrelated baseline failures, and absent build artifacts). No failure names this change.
- The local skill collection provides `om-auto-review-pr` instructions but no executable command; the PR remains draft/in-progress for the reserved independent review.

## Risks

The bind host may be an IPv6 literal or an unspecified wildcard address; URL formatting must bracket IPv6 literals, while the default must remain dialable loopback. The probe must use the exact configured bind host so a second cockpit cannot claim the same port on a different interface.

## Progress

> Convention: `- [ ]` pending, `- [x]` done. Append ` — <commit sha>` when a step lands. Do not rename step titles.

### Phase 1: Reproduce and implement

- [x] 1.1 Extract/test host-aware API URL formatting and port-probe host selection. — 0b5dc705
- [x] 1.2 Wire the helpers into cockpit startup and preserve loopback defaults. — 0b5dc705

### Phase 2: Validate and publish

- [ ] 2.1 Run red-before-fix and green regression tests, then the configured validation gate.
- [ ] 2.2 Run the authoritative PR review/autofix pass and finalize the PR.
