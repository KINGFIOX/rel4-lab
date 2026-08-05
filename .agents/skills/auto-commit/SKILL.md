---
name: auto-commit
description: Always create a safe git commit and push it after Codex finishes a coding task in this project. Use for every implementation, code-editing, refactoring, formatting, documentation-editing, or generated-file task unless the user explicitly says not to commit or not to push, and stage only task-scoped changes after relevant verification.
---

# Auto Commit

## Overview

Use this workflow as the default final step of coding tasks in this project. Protect the user's worktree by inspecting existing changes, staging only changes made for the current task, avoiding commits when verification fails unless the user explicitly accepts that risk, and pushing only safe task-owned commits.

Do not wait for an explicit `$auto-commit` invocation during coding tasks. Treat commit creation and push as always on unless the user says not to commit or not to push, asks for analysis/planning/review only, or a refuse/pause condition applies.

## Workflow

1. Establish the baseline before editing:
   - Run `git status --short`.
   - Treat pre-existing modified, staged, or untracked files as user-owned unless they are clearly part of the requested task.
   - If user-owned changes overlap files needed for the task, read the relevant diffs and work with them. Ask only when committing safely would be ambiguous.

2. Implement the requested code change:
   - Follow the repository's normal coding instructions.
   - Keep edits limited to the task.
   - Do not modify generated, vendored, or unrelated files just to make the worktree clean.

3. Verify before committing:
   - Run the smallest relevant formatter, linter, test, or build command for the change.
   - For Rust workspaces, prefer `cargo fmt --all --check` and the narrowest useful `cargo check` or project-specific test command.
   - If verification cannot run, explain why in the final response and do not claim the commit is verified.

4. Review the exact commit contents:
   - Run `git status --short`.
   - Run `git diff` for unstaged changes you intend to commit.
   - Run `git diff --cached` if anything is already staged.
   - Decide the task-owned file set explicitly. Include new files only when created for the task.

5. Stage only task-owned changes:
   - Use path-limited `git add` commands.
   - Avoid `git add .` unless the worktree contains only task-owned changes and that has been checked.
   - If a file contains both user-owned and task-owned hunks, use an interactive or patch-based staging strategy. If that is impractical, ask the user before committing the mixed file.

6. Commit with a focused message:
   - Use a short imperative subject that names the changed behavior.
   - Match the repository's commit style when visible from recent history.
   - Do not add a long body unless it clarifies non-obvious risk, migration, or test context.

7. Push the commit:
   - Prefer `git push` to the current branch's configured upstream.
   - If the branch has no upstream and there is a single obvious `origin` remote, use `git push -u origin HEAD`.
   - Never force-push, force-with-lease, delete remote refs, rewrite history, or run automatic merge/rebase/pull to satisfy a rejected push unless the user explicitly asks.
   - If push is rejected because the remote has new commits, stop and report that the local commit was created but not pushed.

8. Report the result:
   - Include the commit hash and subject.
   - Include the push destination and outcome.
   - List the verification commands run and their outcomes.
   - Mention any uncommitted user-owned changes left in the worktree.

## Refuse Or Pause Conditions

Do not create a commit when:

- The user asked for analysis, planning, review, or explanation only.
- The user explicitly asked not to commit, to leave changes unstaged, or to stop before committing.
- Verification failed and the user did not explicitly ask to commit despite failure.
- The task-owned changes cannot be separated from unrelated user-owned changes.
- The repository is in the middle of a merge, rebase, cherry-pick, bisect, or other interrupted git operation and the user did not ask to resolve it.
- The commit would include secrets, credentials, generated build artifacts, dependency caches, or vendored code unrelated to the task.

Do not push when:

- The user explicitly asked not to push.
- There is no configured upstream and no single obvious `origin` remote.
- The push would require force, history rewrite, remote branch deletion, merge, rebase, or conflict resolution.
- The push is rejected by the remote; report the rejection and leave the local commit intact.
