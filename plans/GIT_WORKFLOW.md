# Meh — Git Workflow

Each implementation step follows a strict branch-based workflow with user checkpoints.

## Branch Naming
```
develop-step-XX    (e.g., develop-step-01, develop-step-14, develop-step-37)
```

## Step Lifecycle — MANDATORY SEQUENCE

```
┌─────────────────────────────────────────────────────────────────┐
│  1. START        Create branch from latest main                  │
│  2. IMPLEMENT    Write code following plans/STEPXX.md            │
│  3. VALIDATE     Run quality gate (fmt + clippy + test)          │
│  4. PRESENT      Show results to user, explain what was done     │
│  5. FEEDBACK     User reviews — may request changes              │
│  6. ITERATE      Fix issues, re-validate (loop 3-5 until OK)    │
│  7. USER SAYS    "done" / "commit" / "raise PR"                  │
│  8. FINALIZE     Update plans/STEPXX.md acceptance checkmarks    │
│  9. COMMIT       Stage files, commit with conventional message   │
│ 10. PR           Push branch, create PR via gh cli               │
│ 11. TRACKER      Update plans/TRACKER.md, push to PR branch     │
│ 12. WAIT         User merges PR                                  │
│ 13. CLEANUP      Checkout main, pull, delete branch              │
└─────────────────────────────────────────────────────────────────┘
```

## Detailed Steps

**1. START** — Create branch from main
```bash
git checkout main && git pull origin main
git checkout -b develop-step-XX
```

**2. IMPLEMENT** — Write all code, tests, and docs per `plans/STEPXX.md`

**3. VALIDATE** — Run quality gate (MUST pass before presenting to user)
```bash
cargo fmt --check && cargo clippy -- -D warnings && cargo test
```

**4. PRESENT** — Tell the user what was implemented and test results

**5–6. FEEDBACK LOOP** — User may request changes. Fix and re-validate until user is satisfied.

**7. USER CONFIRMS** — Wait for explicit "done", "commit", "raise PR", or similar confirmation. **NEVER commit or push without user confirmation.**

**8. FINALIZE** — Update `plans/STEPXX.md`:
- Check all acceptance criteria boxes (`- [ ]` → `- [x]`)
- Add `**Completed**: PR #N` at the bottom
- Update any criteria text if implementation deviated (e.g., test counts)

**9. COMMIT** — Stage specific files (never `git add -A`)
```bash
git add <specific files>
git commit -m "$(cat <<'EOF'
feat(step-XX): <short description>

<detailed description of what was implemented>

Implements: plans/STEPXX.md

Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com>
EOF
)"
```

**10. PR** — Push and create PR
```bash
git push -u origin develop-step-XX
gh pr create --title "Step XX: <short title>" --body "$(cat <<'EOF'
## Summary
<bullet points>

## Step Reference
See [plans/STEPXX.md](plans/STEPXX.md) for full specification.

## Quality Gate
- [x] `cargo fmt --check` passes
- [x] `cargo clippy -- -D warnings` passes
- [x] `cargo test` passes (N tests)
- [x] Zero compiler warnings
- [x] All acceptance criteria from STEPXX.md met

## Test plan
<specific tests that were added/verified>

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

**11. TRACKER** — Update `plans/TRACKER.md`:
- Set step status: `[x]` with PR number
- Update phase summary counts
- Commit and push to the same PR branch

**12. WAIT** — User merges the PR (never merge yourself)

**13. CLEANUP** — After user confirms merge:
```bash
git checkout main && git pull origin main
git branch -d develop-step-XX
```

## Rules
- **One branch per step** — never mix multiple steps in one branch
- **Branch from main** — always start from latest main
- **NEVER commit, push, or create PR without user's explicit confirmation**
- **NEVER merge PRs** — that is the user's responsibility
- **Always update step file checkmarks AND tracker** before committing
- **Commit messages** use conventional commits: `feat(step-XX):`, `fix(step-XX):`, `test(step-XX):`
- **PR title** format: `Step XX: <short description>`
- **Quality gate in PR** checkboxes should be `[x]` (checked) since they were verified before commit
