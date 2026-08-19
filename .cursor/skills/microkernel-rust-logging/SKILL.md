---
name: microkernel-rust-logging
description: Use when adding, changing, instrumenting, debugging, or reviewing Rust runtime logging in this microkernel's kernel or userspace crates; require Rust log macros instead of println-style logging.
---

# Microkernel Rust Logging

## Policy

Runtime diagnostics in this repository must go through the Rust `log` facade.
Use `error!`, `warn!`, `info!`, `debug!`, or `trace!` for kernel and userspace
runtime messages.

Do not add `println!`, `crate::println!`, `print!`, ad hoc UART writes, or
direct console writes for logging or temporary diagnostics. Temporary debug
instrumentation must follow the same rule and should be removed before final
validation unless it is part of the requested behavior.

## Where To Import

- In kernel modules, use the existing logging facade imports such as
  `use log_crate::{debug, error, info, trace, warn};` or the crate-level
  re-exports when already used nearby.
- In userspace crates, import the macros exposed by `sel4_user`, such as
  `use sel4_user::{debug, error, info, trace, warn};`, or local utility
  re-exports already established in that crate.
- Match nearby import style and only import the levels actually used.

## Exceptions

`build.rs` files may use `println!("cargo:...")` for Cargo build-script
protocol output. That is not runtime logging. Do not treat those Cargo
directives as violations unless they are being used for human diagnostic logs.

Kernel panic output and very early boot paths may have low-level console
primitives already present. Do not add new low-level print paths for ordinary
logging; prefer moving diagnostics onto the initialized `log` path when the
code can run after logger initialization.
