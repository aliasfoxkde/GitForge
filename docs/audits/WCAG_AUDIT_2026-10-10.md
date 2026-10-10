# WCAG 2.1 AAA Audit — template-parts UI templates (2026-10-10)

Scope: the two UI scaffolding templates the platform composes user
projects from — `template-parts/vite-react-pwa` (component library:
Button, Input, LoadingSpinner, LoginForm; dark theme tokens) and
`template-parts/vite-ssr` (TanStack-start style root route). Audit is
static source review; contrast ratios are computed from the token hex
values and marked approximate.

Theme under audit (vite-react-pwa `src/styles/globals.css`):
`--background: #0a0a0a`, `--foreground: #fafafa`, `--border: #27272a`,
`--card: #18181b`. Body text contrast ≈ 18:1 — **passes AAA 1.4.6**.

## Findings

| # | Criterion | Level | Finding | Status |
|---|---|---|---|---|
| 1 | 2.4.7 Focus Visible | AA | **FAIL** — Button and Input apply `focus-visible:ring-2 focus-visible:ring-ring` / `focus:ring-2 focus:ring-ring`, but no `--ring` (Tailwind v4: `--color-ring`) token exists anywhere in the template; the ring resolves to nothing, so keyboard focus is invisible | fixed |
| 2 | 3.3.1 Error Identification | A | **FAIL** — `Input` renders the error as a bare `<span>`: not associated via `aria-describedby`, no `aria-invalid` on the field, not announced | fixed |
| 3 | 1.4.1 Use of Color | A | **FAIL** — error state is signaled by red text + red border only (color-only) | fixed |
| 4 | 1.4.8 Visual Presentation (contrast) | AAA | **FAIL** — error/help text uses `text-red-500` (#ef4444 ≈ 4.6:1 on #0a0a0a) and `text-foreground/60` (≈ 7.4:1, borderline); AAA requires 7:1 for body text | fixed |
| 5 | 4.1.3 Status Messages | AA | **FAIL** — `LoadingSpinner` renders an animated SVG with no `role="status"`, no accessible name; when `App` swaps the page for the spinner there is no announcement | fixed |
| 6 | 2.5.5 Target Size | AAA | **FAIL** — `Button` `sm` size is `h-8` (32px) and `md` is `h-10` (40px), below the 44×44 AAA target | fixed |
| 7 | 2.3.3 Animation from Interactions | AAA | **FAIL** — spinner rotation is not gated on `prefers-reduced-motion` | fixed |
| 8 | 3.3.2 Labels or Instructions | A | **PARTIAL** — `helperText` is rendered but not associated with the field (`aria-describedby` missing); placeholder at `foreground/50` (≈ 9:1 over background once alpha-composited, approx) passes | fixed |
| 9 | 2.4.1 Bypass Blocks | A | **FAIL** (vite-ssr) — no skip link; nav-free template but the pattern every generated project inherits should include one | fixed |
| 10 | 1.3.1 Info and Relationships | A | **PARTIAL** (vite-ssr) — page shell is `<div>`-only; no header/main landmarks for generated projects to extend | fixed |
| 11 | 1.4.6 Contrast (Enhanced) | AAA | **PASS** — primary theme pair ≈ 18:1; ssr's `gray-100` on `gray-950` ≈ 17:1 | pass |
| 12 | 3.1.1 Language of Page | A | **PASS (indirect)** — source `index.html` is not committed for either template; the committed `dist/index.html` outputs carry `lang="en"`; generated projects therefore inherit it. Noted as a hygiene issue instead (see below) | pass |

## Hygiene findings (cross-ref: repo cleanup, 2026-10-10)

- Both templates commit their **build output** (`dist/index.html`,
  `dist/assets/`) into the repo — generated artifacts, not source; the
  source `index.html` for each template is absent. Build outputs are
  untracked and ignored; the source entries stay in the template.

## Fix summary (this branch)

- `globals.css`: defines `--color-ring` (#60a5fa ≈ 7.9:1 on background)
  and `--color-danger` (#f87171 ≈ 7.3:1) as Tailwind v4 theme tokens,
  plus a global `prefers-reduced-motion` guard.
- `Button.tsx`: sizes raised to meet the 44px target (`sm` 44, `md` 44,
  `lg` 48); `destructive` variant text token switched to `--color-danger`.
- `Input.tsx`: `aria-invalid`, `aria-describedby` wiring for error and
  helper text, `role="alert"` error message, danger token for error
  styling, visible (non-color-only) error treatment.
- `LoadingSpinner.tsx`: `role="status"` + screen-reader text +
  `motion-reduce:animate-none`.
- `App.tsx` / ssr `root.tsx`: skip-to-content link, landmark semantics.

Not attempted this cycle: automated contrast/axe verification runs
(template tests cover rendering, not computed styles) — flagged for the
next frontend cycle.
