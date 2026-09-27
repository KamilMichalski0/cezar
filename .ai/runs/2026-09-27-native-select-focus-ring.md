# Execution plan — native `<select>` focus ring

Slug: `native-select-focus-ring` · Date: 2026-09-27 · Base: `main`

## 🎯 Goal

Every native `<select>` in the cockpit focuses with the cockpit's own ring (lime on dark,
ink on light) instead of the browser's default blue outline.

## Context

The reported control is the dashboard's **Tasks created** period picker
(`packages/web/src/routes/dashboard/costs.tsx:225`). Its class list is
`min-h-11 rounded-md border bg-background px-3 text-sm` — no focus treatment at all, so
Chrome paints `outline: auto -webkit-focus-ring-color`, a blue ring. Two rules make that a
defect rather than a preference:

- `.ai/specs/2026-07-14-cockpit-ui-redesign.md:272` — "focus-visible rings everywhere
  (ink light / lime dark)".
- `packages/web/src/styles/index.css` (dark token block) — "nothing else in the cockpit is
  blue"; blue is reserved as `--info` ink for "waiting on a person". A blue ring on a
  filter control borrows a colour that already means something else.

The house spelling already exists at six sites (e.g.
`packages/web/src/routes/settings/resources-section.tsx:194`):
`outline-none focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50`.

A scan of every native `<select>` in `packages/web/src` found eight that lack it, so this is
a class of defect, not one control. Fixing only the reported one leaves the same blue ring on
its three dashboard siblings — including the *Sort by* select sitting a few pixels away.

## Scope

- Apply the house focus treatment to the eight native `<select>`s that lack it.
- Add a design-guardian rule so a new native `<select>` cannot ship without it.

### Non-goals

- The Radix `components/ui/select.tsx` primitive (already correct, not a native select).
- Background/border token changes (`bg-background` → `bg-card`, `border` → `border-input`).
  `--border` and `--input` are the same value in both themes, and the dashboard's inset
  `bg-background` on a card is deliberate contrast. Churn without a behaviour change.
- Inputs, buttons and other controls — none were reported and none were found bare.

## Implementation Plan

### Phase 1 — restore the ring on every bare native select

Sites (verified by a brace-aware scan of opening tags, comments stripped):

| File | Line | Today |
| --- | --- | --- |
| `routes/dashboard/costs.tsx` | 64 (Sort by) | no focus classes |
| `routes/dashboard/costs.tsx` | 225 (Tasks created — reported) | no focus classes |
| `routes/dashboard/overview.tsx` | 88 (Outcomes period) | no focus classes |
| `routes/dashboard/trends.tsx` | 239 (Period) | no focus classes |
| `routes/github/github.tsx` | 1060 (Merge method) | no focus classes |
| `routes/github/github.tsx` | 1146 (Select changed file) | no focus classes |
| `routes/settings/add-account-dialog.tsx` | 107 (Agent) | `outline-none` + border tint only — worse than the default: the ring is suppressed and nothing replaces it |
| `routes/tracker/tracker.tsx` | 126 (Issue state) | no focus classes |

### Phase 2 — pin it with a guardian rule

`packages/web/src/design-guardian.test.ts` already fails the validation gate on design-system
violations with a file and line. Its `RULES` are line-level *forbidden-token* regexes; this is a
multi-line *required-token* check over a JSX opening tag, so it lands as its own `it` with a
small brace/quote-aware tag scanner, resolving a `className={someConst}` reference to the
const's declaration (how `components/dispatch-toggle.tsx` spells it).

### Phase 3 — validation

Full `validation.commands` gate.

## Risks

- **Low.** Class-string additions with no layout impact: `ring` is a box-shadow in Tailwind v4,
  so nothing reflows, and `outline-none` only removes the outline these controls should not have
  been painting. No UI screenshot is possible on this host (no browser can launch), so the visual
  claim rests on the token definitions and on six existing call sites rendering the same classes.
- The guardian's identifier resolution is a heuristic (a `const` declared within three lines of
  its ring classes). It can only *over*-report, which fails loudly rather than silently.

## Progress

> Convention: `- [ ]` pending, `- [x]` done. Append ` — <commit sha>` when a step lands. Do not rename step titles.

### Phase 1: Restore the ring on every bare native select

- [ ] 1.1 Dashboard selects (costs ×2, overview, trends)
- [ ] 1.2 GitHub, tracker and add-account selects

### Phase 2: Pin it with a guardian rule

- [ ] 2.1 Add the native-select focus-ring rule to the design guardian
- [ ] 2.2 Prove the rule fails without the Phase 1 fix

### Phase 3: Validation

- [ ] 3.1 Full validation gate
