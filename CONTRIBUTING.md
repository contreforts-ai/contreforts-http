# Contributing

This crate is a **submodule** of [`dslabs/erp-sync`](https://labs.deepthought-solutions.net/dslabs/erp-sync).
It was extracted from `crates/contreforts-http` with `git subtree split`, so its history is
the real history of that directory.

It is **not independently buildable**: the manifest inherits `version`, `edition`
and its dependency versions from the parent workspace
(`version.workspace = true`, `contreforts-core = { workspace = true }`). Build and test it
from a checkout of the superproject, not from a standalone clone.

---

## 1. Branch model

| Branch | Role |
|---|---|
| `develop` | Default and only long-lived branch. Protected — **no direct pushes**. |
| `<author>/<kind>/<slug>` | Short-lived working branches, deleted automatically on merge. |

`<kind>` is the conventional-commit type the branch is mostly about:
`feat`, `fix`, `refactor`, `docs`, `chore`, `test`, `security`.

```
nkarageuzian/feat/17-per-user-store
llazare/fix/1-pennylane-429-retry
```

There is no `main` and no release branch. Tags mark releases.

## 2. Merge policy — strict rebase + merge commit

Every change lands through a pull request. The **only** enabled merge style is
*Rebase then create merge commit* (`rebase-merge`, i.e. rebase onto `develop`
then merge with `--no-ff`):

| Style | Enabled |
|---|---|
| Rebase then create merge commit (`--no-ff`) | ✅ **the only option** |
| Create merge commit | ❌ |
| Squash merge | ❌ |
| Rebase and fast-forward | ❌ |
| Fast-forward only | ❌ |

This keeps a linear, bisectable commit sequence while preserving an explicit
merge commit per PR, so each PR remains a single revertable unit.

Source branches are deleted after merge (`default_delete_branch_after_merge`).

**Keep your branch current by rebasing, never by merging `develop` into it:**

```bash
git fetch origin
git rebase origin/develop
git push --force-with-lease
```

`--force-with-lease` (not `--force`) so you never clobber someone else's push.

## 3. Commits

- **One commit per completed task** — commit before moving on.
- Conventional prefixes: `feat:`, `fix:`, `test:`, `refactor:`, `docs:`,
  `chore:`, `security:`.
- Reference issues in the body: `Refs #18`, `Closes #18`. Issues live on the
  superproject tracker: https://labs.deepthought-solutions.net/dslabs/erp-sync/issues

### Bug fixes: TDD via stash-and-replay

Produces a two-commit trail where the `test:` commit pins the spec independently
of the implementation, making `git bisect` meaningful.

1. Implement the fix, then set it aside:
   ```bash
   git stash push -m <fix-slug> -- <files-changed>
   ```
2. With the tree back in the buggy state, write a test that captures the spec of
   the fix (a compile error against a not-yet-existing symbol counts). Commit it
   alone, intentionally red:
   ```bash
   git commit -m "test: <the broken behaviour>"
   ```
3. Replay the fix, confirm green, commit:
   ```bash
   git stash pop && cargo test -p <crate>
   git commit -m "fix: <the fix>"
   ```

## 4. Working on this crate from the superproject

The submodule is a full clone — edit in place under `crates/contreforts-http/`, but mind
that a fresh submodule checkout is in **detached HEAD**.

```bash
# 1. Get on a branch (never commit detached)
cd crates/contreforts-http
git checkout develop && git pull

# 2. Work
git checkout -b <author>/<kind>/<slug>
#    …edit, test from the superproject root: cargo test -p contreforts-http
git commit -m "feat: …"
git push -u origin HEAD
#    open a PR against develop, get it merged

# 3. Record the new commit in the superproject
cd ../..
git submodule update --remote crates/contreforts-http
git add crates/contreforts-http
git commit -m "chore(deps): bump contreforts-http to <short-sha>"
```

Step 3 is not optional: the superproject pins an exact commit, so an unbumped
pointer means the merged work is invisible to everyone cloning erp-sync.

### Changes spanning several crates

A change touching `contreforts-core` and its consumers is **N+1 pull requests** —
one per affected submodule, then one on the superproject bumping all the
pointers together. Merge the `contreforts-core` PR first, then the consumers, then
bump. The superproject bump commit is the atomic point: before it, nobody sees
a half-applied change.

## 5. Where to file issues

This repo and the superproject both have trackers. They are **not** interchangeable.

| File it in | When |
|---|---|
| **this repo** | The work is confined to this crate's internals — its own refactors, defects, public API. |
| **`dslabs/erp-sync`** | Everything else: cross-crate work, architecture and product features, CI, release, the JS packages, and anything touching the in-tree crates (`erp-cli`, `erp-api`, `erp-ffi`, `apps/desktop`). |

**When in doubt, file it in `dslabs/erp-sync`.** That is the tracker everyone watches; moving an issue down into a crate later is cheaper than it sitting here unseen.

Cross-repo work gets a **parent issue in the superproject and one sub-task per affected repo**. Link them with fully-qualified refs — `dslabs/erp-sync#15`, `contreforts/contreforts-core#1` — because a bare `#N` resolves to whichever repo you are reading. This Forgejo build exposes **no issue-dependencies API**, so ordering between sub-tasks lives in the issue text (`Blocked by:` / step *n* of *m*), not in structured metadata.

Every issue carries exactly one `severity:` and one `kind:` label, from the same 10-label taxonomy as the superproject.

## 6. Rust conventions

- `async` traits go through `async-trait`.
- `thiserror` for library error types, `anyhow::Result` at call sites.
- Structured logging with `tracing` — never `println!`.
- No `unwrap()` in library code; use `?` or handle explicitly.
- Before pushing:
  ```bash
  cargo fmt --all
  cargo clippy --workspace --all-targets -- -D warnings
  cargo test -p contreforts-http
  ```

## 7. Data Access Policy

**Only** the sync process (adapter `pull` calls) may fetch from external APIs.
All reporting, dashboarding and browsing **must** read the central Oxigraph
store via SPARQL. External hierarchy (e.g. GitLab groups/projects) is persisted
into the graph during sync, never queried on demand.
