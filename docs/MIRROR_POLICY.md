# Mirror and CI/CD platform policy

Status: normative. Applies to every project on this host and to every
agent session working in this workspace. First written 2026-09-24 after
the third "pipeline red on GitHub, code is fine" confusion in one week.

## GitForge is the platform, GitHub is a mirror

- **CI/CD, pipelines, releases, and gates run on GitForge**
  (`/nas/Temp/repos/GitForge`, API `:42780`, git HTTP `:42782`).
  Branch protection, the release gate
  (`scripts/gitforge-release-gate`), and coverage gates live there.
- **GitHub is a backup/sync mirror.** A red GitHub Actions run is *not*
  a code failure signal: the mirror account is billing-blocked, so
  Actions cannot run. Do not open "fix CI" PRs, do not debug billing,
  and do not treat an Actions red as a regression. The GitHub mirror is
  also public: nothing that cannot be published belongs on it (see
  F22 — the tracked `.env` published a live `JWT_SECRET` for weeks).

## Remotes

Projects keep both remotes and push **GitForge first**, then GitHub:

```
gitforge   http://localhost:42782/<owner>/<repo>.git   (or LAN address)
origin     https://github.com/<owner>/<repo>.git        (mirror)
```

A push to GitForge triggers that project's `.gitforce.yml` pipeline.
If a push to GitForge succeeds and the GitHub mirror push fails, the
mirror may be repaired later; the reverse is never acceptable.

Divergent mirrors are not reconciled unilaterally. Known case:
`gitforge-ci`'s mirror `main` tracks codex's integration line; force
pushes are forbidden (`--force-with-lease` included — use delete +
re-push only after confirming no one else base-lined the old head).

## Merges

- Merges to a project's `main` go through a GitForge pipeline green on
  the PR branch **and** (for GitForge itself) a green run of the merge
  commit itself — `gitforge-release-gate` refuses a cut without it.
- GitHub's rulesets are bypassed only with the temporary-grant
  procedure: PUT ruleset bypass with actor, `gh pr merge --admin`,
  immediately PUT restore (`[]`), verify the restore landed. Bypass is
  a two-API-call window, never a standing state.
- A GitHub merge does not deploy anything. Deployment is the GitForge
  release bundle → promote → drain-gated restart sequence
  (`docs/RUNBOOK.md`).

## What belongs where

| Concern | Home |
| --- | --- |
| Pipeline definition | `.gitforce.yml` in the project repo |
| Coverage / lint gates | `.gitforce.yml` jobs (e.g. GitForge at 82% fail / 84% warn, calibrated in-sandbox — F28) |
| CI images | `infrastructure/docker/*.Dockerfile`, pre-built, tag-bumped |
| Release evidence | GitForge run of the exact source commit |
| Secrets | `gitforge auth --login` (interactive) or credential files; never repo files, never the mirror |

## History

- 2026-09-22 — standing directive: move all pipelines to GitForge.
- 2026-09-24 — policy written; GitForge's own pipeline gained the
  coverage gate in PR #229 (v0.6.8 line).
- 2026-09-24 — v0.6.8 tagged at merge 47a3ac43 and deployed
  (release `gitforge-47a3ac43-20260924`, gate run 88148271). The
  coverage baseline is what the pipeline itself measures on the tag:
  **83.06% lines** (82 hard gate / 84 advisory), replicated at 83.02%
  on the follow-up branch — the gate's evidence is the durable run,
  not a separate sweep.
