# Docs

These files describe the tree as it is compiled today. The source of truth is
the first-party Rust and Python in `kernel/`, `userspace/`, and `tools/`. They
do not record historical QEMU runs or retired milestone banners.

| File | What it describes |
|------|-------------------|
| [kernel.md](kernel.md) | Rust kernel: objects, ABI, scheduler, arch split, boot. |
| [userspace.md](userspace.md) | seL4 user libraries and the xv6 server stack. |
| [fpu.md](fpu.md) | RISC-V FPU ownership and the source audits that check it. |
| [tools.md](tools.md) | First-party pack, QEMU, xv6, and ABI audit helpers. |

Commands live in the root [README.md](../README.md). Policy for agents lives
in [AGENTS.md](../AGENTS.md) and `.cursor/skills/`.
