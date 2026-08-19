---
name: rust-style
description: Follow the linked official Rust Style Guide when editing Rust code. Use for every Rust coding, refactoring, formatting, API design, doc-comment, no_std, kernel, userspace, or Cargo-related change in this project, while preserving local repository conventions and relying on rustfmt-compatible style.
---

# Rust Style

## Overview

Apply the official Rust Style Guide for Rust edits in this project. Prefer `rustfmt` output as the source of truth, then manually keep names, comments, modules, and API shape consistent with the surrounding code.

## Workflow

1. Follow local style first where it is more specific:
   - Respect nearby module organization, naming patterns, unsafe invariants, error handling, and no_std constraints.
   - Do not introduce broad refactors only to restyle unrelated code.
   - Preserve public API names and ABI-sensitive layout unless the task requires changing them.

2. Format with Rust's standard style:
   - Use `cargo fmt --all` when the task allows modifying all formatted Rust files.
   - Use narrower formatting only when unrelated user-owned changes are present and whole-workspace formatting would touch them.
   - If a repository-local `rustfmt.toml` or `.rustfmt.toml` appears later, follow it.

3. Keep code idiomatic and readable:
   - Use `snake_case` for functions, variables, modules, and fields.
   - Use `CamelCase` for types and traits.
   - Use `SCREAMING_SNAKE_CASE` for constants and statics.
   - Prefer clear names over abbreviations, except for established architecture or kernel terms used nearby.
   - Keep imports organized by `rustfmt`; avoid unused imports.

4. Be deliberate with comments and docs:
   - Add comments only for non-obvious invariants, unsafe preconditions, concurrency ordering, ABI layout, or hardware behavior.
   - For `unsafe` code, document the safety argument when it is not already clear from nearby code.
   - Keep doc comments concise and useful; do not narrate obvious assignments.

5. Validate Rust changes:
   - Run `cargo fmt --all --check` after Rust edits when possible.
   - Run the narrowest useful `cargo check`, package build, or project-specific test command for the changed area.
   - If checks cannot run because dependencies, targets, or external tools are unavailable, report that explicitly.

## Rustfmt Is The Baseline

The linked style guide is the official Rust Style Guide at `https://doc.rust-lang.org/style-guide/`. Treat it as the formatting baseline and let `rustfmt` decide layout for expressions, imports, match arms, chains, generics, attributes, and where clauses.

Manual edits should avoid fighting `rustfmt`. When hand-writing code before formatting, keep lines clear, use trailing commas in multiline lists and patterns where idiomatic, avoid needless blank lines, and keep item order consistent with neighboring modules.

## Project Notes

This repository contains no `rustfmt.toml` at skill creation time, so default `rustfmt` behavior applies. If one is added, treat it as project policy.
