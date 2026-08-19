---
name: microkernel-dev-env
description: Use the already-loaded direnv flake environment for this microkernel repository. Use whenever Codex runs cargo, rustfmt, Rust target builds, seL4 pack/sim/test helpers, QEMU, linux-compat/LTP helpers, or other validation commands here; avoid re-running nix develop unless the current shell lacks the required tools.
---

# Microkernel Dev Env

## Overview

Run validation from the current project shell first. This repository's `.envrc` uses `use flake`, and Codex may already be running inside a direnv-allowed flake environment with the Rust and QEMU tooling on `PATH`.

## Workflow

1. Prefer direct commands from the current shell:
   - Run `cargo`, `rustfmt`, `cargo fmt`, `cargo check`, `tools/*.py`, QEMU helpers, seL4 test helpers, and linux-compat helpers directly when they are already available.
   - Do not prefix routine validation with `nix develop` by default.
   - Do not spend time entering a new Nix shell before every formatter, check, build, seL4 test, or LTP run.

2. Check the environment only when needed:
   - Use `command -v cargo`, `command -v rustfmt`, `command -v qemu-system-riscv64`, or the specific missing tool to verify availability.
   - Use `direnv status` when the environment looks wrong or a required tool is missing.
   - Treat tools under `/nix/store/...` as evidence that the flake environment is already active.

3. Fall back in this order when a required tool is unavailable:
   - Try `direnv exec . <command>` if `direnv` is installed and the repo `.envrc` is allowed.
   - Use `nix develop -c <command>` only when direct execution and `direnv exec` are unavailable or fail because the flake environment is not active.
   - Report clearly when a fallback is used because it may be slower.

4. Use task-appropriate validation:
   - For Rust formatting, prefer `cargo fmt --all --check` after edits.
   - For Rust type checking, run the narrowest useful `cargo check` or package-specific check for the changed area.
   - For kernel image or seL4 regressions, use `tools/pack-image.py` and `tools/run-tests.py` with targeted filters when possible.
   - For linux-compat behavior, use `TIMEOUT=180 ARCH=riscv64 tools/run-ltp.py` or `TIMEOUT=180 ARCH=x86_64 tools/run-ltp.py`.
   - Use longer QEMU runs only when they cover the changed behavior.

## Current Repo Assumption

At skill creation time, `cargo` and `rustfmt` resolved from `/nix/store/...`, and `direnv status` showed the repository `.envrc`, `flake.nix`, and `flake.lock` as loaded and allowed. Re-check only when command failures suggest the environment changed.
