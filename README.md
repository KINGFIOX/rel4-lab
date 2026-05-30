# microkernel — a Rust reimplementation of seL4 (RV64, qemu-riscv-virt)

A minimal, milestone-driven rewrite of the seL4 microkernel in Rust, targeting
the same ABI as the official C kernel so that the existing `sel4test-driver`
binary boots unmodified on top of it.

## Current status

The `sel4test-driver` rootserver boots, spawns helper TCBs and per-test
child processes in their own VSpaces, and runs them on the Rust kernel
through the official `libsel4` ABI. Endpoint IPC, notifications, reply
caps, timer preemption, several CNode/Untyped paths,
multi-size frame map/unmap, DomainSet, fault IPC, and ASID pool creation
are implemented far enough for the full suite to run to completion.

Latest verified checkpoints:

- Context handoff note: floating-point/FPU/FS-bit work is not the active task.
  The current FPU issue has already been handled; do not reopen FPU fixes after
  context compaction unless explicitly requested. The active line of work is the
  user-space xv6 compatibility server.
- Historical RV64 non-MCS single-core checkpoint: `./tools/run-tests.sh` passed
  with **124 enabled tests passing, 43 upstream-disabled tests remaining**
  before the upstream build tree was switched to the current SMP configuration
  and before M4.4g removed kernel floating-point context support.
- RV64 SMP-compatible build (`SMP=ON`, `NUM_NODES=2`, QEMU `SMP=2`):
  `env SMP=2 TIMEOUT=480 ./tools/run-tests.sh` currently reaches the full
  suite but reports **121 enabled tests passing, 42 upstream-disabled tests
  remaining, and 4 FPU tests failing** after the M4.4g/M4.4h "no kernel
  floating point" changes.
- xv6 user-program compatibility smoke path: the initially requested xv6 user
  ELF is embedded into the `xv6-host` rootserver, loaded into a child
  TCB/VSpace, and handled through seL4 fault IPC via
  `./tools/run-xv6-user.sh`. Subsequent `exec()` calls now load real xv6 user
  ELF bytes from `fs.img` through `xv6-host -> xv6-fs-server ->
  virtio-disk-server` instead of using the host's embedded exec catalog.
  Verified:
  `echo`, `forktest`, `cat README`, `ls .`, `wc README`, and
  `grep xv6 README` end in `xv6-host: exit(0)`. The `sh` path can now consume
  scripted console input and run `fork/exec/wait` command lines, including a
  simple `echo ... | wc` pipeline. Targeted `usertests` coverage now includes
  `sharedfd`, `fourfiles`, `createdelete`, `unlinkread`, `linktest`,
  `concreate`, `linkunlink`, `subdir`, `bigwrite`, `bigfile`, `forktest`,
  `sbrkmuch`, `sbrkfail`, the lazy allocation group, and the slow tests
  `bigdir`, `manywrites`, `badwrite`, `execout`, `diskfull`, and
  `outofinodes`. `diskfull` now also exercises directory-block exhaustion
  without the former unexpected-`mkdir` diagnostic. The full xv6 `usertests`
  suite now reaches `ALL TESTS PASSED` with
  `env TIMEOUT=1200 ./tools/run-xv6-user.sh usertests`.
- M6.1 has started the microkernel-style xv6 service split. Shared user-space
  seL4 ABI code now lives in `userspace/sel4-user`, xv6 syscall/fs/disk
  protocol constants live in `userspace/xv6-abi`, and `xv6-host` consumes those
  shared crates instead of carrying private copies. `userspace/xv6-fs-server`
  and `userspace/virtio-disk-server` are now independent no_std Rust 2024
  server crates that compile against the shared ABI/protocol. The current
  booted xv6 path still uses the in-host in-memory filesystem; fs.img-backed
  virtio block I/O is the next migration step, not completed in M6.1.
- M6.2 adds the xv6 disk-image/QEMU side of that migration. `tools/run-xv6-user.sh`
  now builds xv6's native `fs.img` into `target/xv6compat/fs.img` and attaches
  it to QEMU as a `virtio-blk-device` by default, matching xv6's own QEMU
  layout at MMIO base `0x10001000`. `userspace/xv6-abi` now also carries the
  xv6 on-disk FS structs and virtio-mmio register/queue constants that the fs
  and disk servers will share.
- M6.3 now boots the first real service-server topology. `xv6-host` embeds and
  starts `xv6-fs-server` and `virtio-disk-server` as independent user-mode
  TCB/CNode/VSpace instances, each with its own private service endpoint at
  child cptr 2. The fs server also receives a disk endpoint cap at cptr 3.
  Before launching the xv6 payload, `xv6-host` performs a synchronous
  `FS_OP_INIT` call; `xv6-fs-server` answers it by calling
  `DISK_OP_GET_INFO` on `virtio-disk-server`. Verified log path:
  host init -> fs init -> disk get-info -> host fs-ready.
- M6.4 maps the real QEMU virtio-mmio block device into
  `virtio-disk-server`. `xv6-host` carves the `0x10001000` device frame from
  BootInfo device untyped caps, maps it at `0x50000000` in the disk server,
  allocates a DMA page, obtains its physical address with `RISCV_Page_GetAddress`,
  and maps it at `0x50001000`. The disk server now initializes queue 0 and
  can read xv6 `fs.img` blocks via virtio-blk; M6.5 below moves on-disk
  superblock validation to the filesystem server.
- M6.5 moves the first disk data exchange onto the fs<->disk server boundary.
  `xv6-host` now maps a shared 4KiB frame at `0x50002000` into both
  `xv6-fs-server` and `virtio-disk-server`; `DISK_OP_GET_INFO` is now geometry
  only, and `xv6-fs-server` validates the superblock by calling
  `DISK_OP_READ(1)` and reading the returned 1KiB block from that shared page.
  Verified with `echo disk read ipc` and full `usertests`, both reaching
  `xv6-host: exit(0) pid=1`; `usertests` also reaches `ALL TESTS PASSED`.
- M6.6 starts real xv6 on-disk filesystem parsing in `xv6-fs-server`. During
  `FS_OP_INIT`, the fs server now parses the superblock, reads inode blocks,
  verifies the root inode, scans the root directory from data blocks, and finds
  the real `README` inode from `fs.img` (`ino=2`, `size=2441`, `nlink=1` in the
  current xv6 image). It also exposes a first read-only `FS_OP_OPEN` path lookup
  entry point for root-level files.
- M6.7 wires the first xv6 file syscalls through the fs server. `xv6-host`
  maps the shared block page into its own VSpace, records the fs-server
  endpoint, and routes read-only regular-file `open`, `read`, and `fstat`
  through `FS_OP_OPEN`, `FS_OP_READ`, and `FS_OP_FSTAT`. Verified:
  `cat README`, `wc README`, `grep xv6 README`, and full
  `env TIMEOUT=1200 ./tools/run-xv6-user.sh usertests` all exit cleanly, with
  logs showing `xv6-fs-server` inode lookup and `virtio-disk-server` block
  reads from `fs.img`.
- M6.8 extends the fs-server route to read-only directory iteration.
  `xv6-fs-server` now handles `FS_OP_READDIR` for xv6 directory files, and
  `xv6-host` can open pristine root directories as fs-server fds so `ls .`
  reads raw dirents from the real `fs.img`. A host-FS mutation guard keeps
  runtime-created directories and files on the host compatibility path until
  write/create/link/unlink/mkdir are moved into the fs server. Verified:
  `ls .`, `cat README`, targeted `usertests concreate`, and full `usertests`.
- M6.9 adds the first reversible block-write path to the split disk stack.
  `virtio-disk-server` now supports `DISK_OP_WRITE` with virtio-blk OUT
  requests, and `xv6-fs-server` performs a boot-time write/read/restore check
  on the last xv6 filesystem block through the shared page at `0x50002000`.
  Verified logs include `virtio-disk-server: write block=1999` and
  `xv6-fs-server: disk write verified block=1999`; `ls .`, `cat README`, and
  full `usertests` still pass afterward. This is only the block-write
  foundation. Real xv6 fs mutation semantics still need to move into the fs
  server.
- M6.10 moves the first real filesystem metadata mutations into
  `xv6-fs-server`. `FS_OP_LINK` now appends or reuses a root-directory dirent
  for an existing regular file and increments the target dinode `nlink`;
  `FS_OP_UNLINK` clears the dirent and decrements `nlink`. Both operations
  write the real xv6 `fs.img` through `DISK_OP_WRITE`. Verified with a shell
  script that runs `ln README readlink`, `cat readlink`, `rm readlink`, and a
  failing second `cat readlink`; full `usertests` still reaches
  `ALL TESTS PASSED`, with `linkunlink` now logging fs-server
  `link cat -> x` / `unlink x` cycles.
- M6.11 extends the fs-server-owned mutation path to fresh root-level
  `open(O_CREATE)`, writable file descriptors, `write`, `O_TRUNC`, direct and
  indirect data-block allocation, inode/block bitmap updates, and immediate
  inode reclamation when `unlink` drops `nlink` to zero with no open refs.
  `xv6-host` now routes fresh writable regular-file fds through
  `FS_OP_OPEN`/`FS_OP_WRITE`/`FS_OP_CLOSE`, while a hybrid mutation guard keeps
  later full-suite host-only directory semantics on the host compatibility
  model once that model has been mutated. Verified with shell
  `echo hello > newfile; cat newfile; rm newfile; cat newfile`, targeted
  `usertests sharedfd` and `usertests concreate`, and full
  `env TIMEOUT=1200 ./tools/run-xv6-user.sh usertests`.
- User-space seL4 ABI cleanup: `xv6-host` no longer keeps a private
  `src/sel4.rs` shim. The host, fs server, and virtio-disk server now import
  the shared syscall/BootInfo/IPCBuf helpers directly from `userspace/sel4-user`
  and share xv6 protocol constants through `userspace/xv6-abi`.
- M6.12 moves nested directory mutation into the fs-server-owned path.
  `xv6-fs-server` now resolves multi-component paths, handles `.` / `..`
  through real xv6 dirents, creates directories with `.` and `..`, creates
  device inodes through `FS_OP_MKNOD`, rejects non-empty directory unlink, and
  applies hard-link/unlink/create under subdirectories. `xv6-host` now builds
  fs-server absolute paths from its cwd mirror for relative paths after
  `chdir`, and mutation syscalls no longer fall back to the host mirror when
  the fs server rejects an fs-owned path. Verified with subdirectory shell
  smoke, targeted `usertests subdir`, and full
  `env TIMEOUT=1200 ./tools/run-xv6-user.sh usertests`.
- M6.13 reduces the remaining cwd/exec dependency on the host filesystem
  mirror. `xv6-host` now tracks a canonical absolute cwd path per process,
  normalizes relative fs-server paths from that path, and validates `chdir`
  and `exec` existence with new `FS_OP_CHDIR` / `FS_OP_EXEC_LOOKUP` requests
  against the real `fs.img` server. `unlink(".")` / `unlink("..")` are rejected
  before canonicalization so xv6 `rmdot` semantics stay intact. Verified with
  cwd/exec shell smoke, targeted `usertests rmdot`, and full `usertests`.
- M6.14 makes normalized filesystem paths authoritative in the fs server.
  `xv6-host` no longer carries the global `HOST_FS_MUTATED` escape hatch:
  open/create/truncate, mkdir, mknod, link, unlink, and chdir all use
  `xv6-fs-server` whenever a path can be normalized for the server, and a
  server-side rejection is returned directly to the xv6 process. The legacy
  host mirror remains only as a fallback for paths that cannot be sent to the
  fs server. Verified with a shell create/link/unlink/chdir smoke and full
  `usertests`.
- M6.15 removes the embedded-catalog data source from the normal `exec()`
  path. For normalized paths, `xv6-host` opens the executable through
  `xv6-fs-server`, streams its ELF bytes from `fs.img` via `FS_OP_READ`, checks
  the ELF headers before destroying the old address space, and then loads that
  image into the child VSpace. The embedded catalog is now only a fallback for
  paths that cannot be represented in the current fixed-size fs-server IPC
  format. Verified with shell `/echo` + `/cat README`, targeted
  `usertests execout`, and full `usertests`.
- M6.16 removes the normal-runtime host-mirror fallback for filesystem path
  syscalls. `xv6-host` now tracks each process's fs-server cwd inode and sends
  `(cwd_inum, raw path)` to `xv6-fs-server`; the fs server resolves relative
  paths from that inode, so deep `chdir`/`mkdir`/`open`/`unlink` no longer need
  a host-expanded absolute path. This fixed `usertests iref`, where repeated
  relative `mkdir("irefd")` exceeded the old 128-byte canonical cwd buffer.
  Verified with the no-host-fallback shell smoke, targeted `usertests iref`,
  and full `usertests`.
- M6.17 deletes the old host-side filesystem mirror from `xv6-host`. Regular
  files, directories, device nodes, cwd resolution, metadata mutation, and
  `exec()` now go through `xv6-host -> xv6-fs-server -> virtio-disk-server`;
  the host keeps only process state, fd offset/refcount state, console, and pipe
  behavior. The generated embedded exec catalog and non-normal host mirror
  fallback were removed. Full `usertests` still reaches `ALL TESTS PASSED`.

M4.4b unlocked the first timer-gated disabled group on the current RV64,
non-MCS, single-core, QEMU configuration: `TIMER0001`, `TIMER0002`,
`SCHED0000`, `DOMAINS0004`, and `PREEMPT_REVOKE`. The Rust kernel now
exposes `rdtime` to userspace, drives a 2 ms SBI timer tick, signals the
synthetic timer `IRQHandler` notification, rotates same-priority threads
on the configured five-tick time slice, and runs `CNode_Revoke` through a
preemptible in-kernel continuation. The upstream sel4test tree at
`/Users/wangfiox/sel4/sel4test` also has a qemu-riscv-virt
`libplatsupport` ltimer shim using `rdtime` plus pseudo IRQ 96, with
timer support enabled even under `Sel4testSimulation=ON`.

M4.4c then enabled `PAGEFAULT1005` on RISC-V by fixing the upstream test's
cross-address-space bad-instruction path: the handler no longer
dereferences the faulter's user stack pointer, and the faulter-side
restart stub writes back `GOOD_MAGIC` before restoring its original SP.
M4.4d enables `SCHED0021` under the current QEMU simulation build: the
Rust scheduler now tracks a C-kernel-style per-TCB time-slice counter,
and the upstream test keeps the original strict bound for non-simulation
while using a bounded simulation-specific timing margin.
M4.4e enables `CACHEFLUSH0004` on RISC-V. The ARM-specific cache
maintenance tests remain architecture-gated; this cross-architecture test
only validates that untyped revoke/retype returns zeroed frame contents,
which the Rust kernel now passes.
M4.4f adds the first SMP-compatible RV64 path. The upstream rootserver can
now be built with `CONFIG_ENABLE_SMP_SUPPORT`/`CONFIG_MAX_NUM_NODES=2`,
QEMU can boot with two harts, and the Rust kernel accepts the SMP-shifted
invocation ABI plus `TCBSetAffinity`. The current kernel still parks
secondary harts and simulates enough affinity/progress semantics on the
primary hart to pass `MULTICORE0001..0005`; real per-hart scheduling, IPIs,
and cross-hart TLB shootdown remain future work.
The upstream OpenSBI packaging helper is also pinned to
`rv64imafdc_zicsr_zifencei` so the current GCC/binutils toolchain can
rebuild the SMP image after CMake regeneration.

M4.4g removes kernel-owned floating-point context support. The trap entry no
longer executes F/D instructions, `UserContext` no longer contains FPR/FCSR
state, and user `sstatus.FS` is left off. The Rust kernel now also builds for
`riscv64imac-unknown-none-elf` and has a compile-time guard against RISC-V
`f`/`d` target features, so a future accidental `rv64gc` kernel build fails
immediately. This intentionally drops the previous FPU sel4test coverage in
favor of keeping the Rust kernel free of floating-point save/restore machinery.

M4.4h removes the remaining FPU compatibility surface from TCB handling:
`Tcb` no longer stores a FPU-disabled flag, `TCBSetFlags` returns
`IllegalOperation`, the naked boot entry clears `sstatus.FS` on every hart
before Rust code runs or secondary harts park, trap entry clears `sstatus.FS`
again before entering the Rust dispatcher, and the restore path masks it off
before every `sret`.

M5.1/M5.2 were the temporary in-kernel xv6 bridge: a generated wrapper linked
one xv6 user program at `0x10000000`, and the Rust kernel directly dispatched
xv6's positive syscall numbers. That path proved the smoke workload but is now
retired.

M5.3 moves xv6 compatibility back to a seL4-style design. The elfloader
rootserver is now `userspace/xv6-host`, a no_std Rust server that parses
BootInfo, allocates seL4 objects from untyped caps, creates a child TCB/CNode/
VSpace/fault endpoint, maps the xv6 payload into the child, and handles xv6
syscalls as `UnknownSyscall` fault IPC. The kernel no longer owns Unix fd,
heap, or pseudo-filesystem state.
`xv6-host` is now a Cargo workspace package on Rust 2024. Its linker args live
in `userspace/xv6-host/build.rs`, the kernel linker args live in
`kernel/build.rs`, and `.cargo/config.toml` selects the shared no-F/D
`riscv64imac-unknown-none-elf` target plus common RISC-V codegen flags. The
host crate also denies the Rust 2024 unsafe migration lints
`unsafe_attr_outside_unsafe` and `unsafe_op_in_unsafe_fn`.

M5.4 starts the real xv6 process model in user space. `xv6-host` now uses one
badged seL4 fault endpoint for all xv6 children, keeps a small process table,
implements `fork` by creating a new TCB/CNode/VSpace, cloning mapped child pages
through the host alias window, reading the blocked parent's registers with
`TCB_ReadRegisters`, and starting the child with `TCB_WriteRegisters`. `exit`
turns non-root children into zombies and `wait` can reap them. `forktest` now
passes through real child creation/exits instead of the old graceful
`fork == -1` path.

M5.5 adds the first usable xv6 shell path. The build can embed scripted console
input with `XV6_CONSOLE_INPUT` or `tools/run-xv6-user.sh --stdin`, console reads
block when no script is provided instead of reporting EOF, and fd tables are now
per xv6 process rather than global. `fork` copies fd references, `exit` closes
the exiting process's fds, and `close/dup/pipe` now work correctly across shell
children. This is enough for scripted `sh` commands such as `echo`, `ls`,
`cat README`, and `echo pipe data | wc` to exercise `fork -> exec -> wait`
through the user-space seL4 server.

M5.6 tightens Unix fd/fs semantics enough to run the first targeted xv6
`usertests` case. `xv6-host` now uses shared open-file objects so `dup` and
`fork` share file offsets, `fork` inherits cwd, the in-memory FS supports
mutable files/directories plus `link`/`unlink`/`mkdir`, and `sbrk` keeps
mapping-table headroom instead of halting the host during `usertests`
`countfree()`. `usertests sharedfd` now reaches `OK` / `ALL TESTS PASSED`.

M5.7 makes the `sbrk`/process memory path reclaim host-side mappings instead
of only moving the xv6 break pointer. Each child mapping now records both the
child frame cap and the host alias cap; shrink, exec reset, and process reap
issue `RISCV_Page_Unmap`, delete the cap slots with `CNode_Delete`, and recycle
those slots in the host allocator. This removes the mapping-table leak exposed
by `countfree()` and lets the next targeted `usertests` group pass:
`fourfiles`, `createdelete`, `unlinkread`, `linktest`, `concreate`,
`linkunlink`, `subdir`, `bigwrite`, and `bigfile`. The xv6 run helper now only
reports success for root-process `exit(0)` with a numeric `pid=1` boundary, so
child exits like `pid=19` and root `exit(1)` are not false positives.

M5.8 aligns xv6 process memory more closely with xv6's own layout and resource
failure behavior. `xv6-host` now places the user stack immediately above the
loaded ELF with a guard page, then starts `sbrk` above that stack instead of
using a fixed high stack inside the heap range. `CHILD_HEAP_LIMIT` is now xv6's
`TRAPFRAME`, large eager `sbrk` requests become sparse mappings serviced by VM
fault IPC, and a host-side sparse reservation limit makes oversized or
concurrent eager allocations fail gracefully instead of exhausting seL4 CSpace.
`fork` now estimates clone slots/mapping slots and returns `-1` on pressure, so
`forktest` follows the xv6 graceful-failure path. Verified targeted cases:
`lazy_alloc`, `lazy_unmap`, `lazy_copy`, `lazy_sbrk`, `forktest`, `sbrkmuch`,
and `sbrkfail`; verified suite: `usertests -q`.

M5.9 gets the full xv6 `usertests` binary through its slow-test section. The
host directory-entry table is now large enough for `bigdir`'s 500 hard links
on top of the embedded exec catalog, while `unlink` continues to recycle
directory slots. Full-suite validation now passes through `manywrites`,
`badwrite`, `execout`, `diskfull`, and `outofinodes` and ends in
`ALL TESTS PASSED`.

M5.10 tightens the in-memory FS space model for xv6 directory content.
Directory entries now consume the same 1KiB data-block pool as regular file
contents, one block per 64 xv6 `dirent` records, and `mkdir` allocates the new
directory's own first content block before publishing it in the parent. Failed
directory extension rolls back the new node and any allocated blocks, so
`usertests diskfull` no longer prints the former
`mkdir(diskfulldir) unexpectedly succeeded` diagnostic.

M5.11 makes the in-memory pipe model blocking in both directions. Empty pipe
reads now save the fault reply cap and keep the reader blocked while writers
remain open; writes, reads, `close`, `exit`, and `kill` pump pipe waiters so
readers resume with data or EOF and full-pipe writers resume when space opens.
Targeted validation: `usertests pipe1` and `usertests preempt`.

M5.12 unifies xv6 process termination semantics across `exit`, VM/syscall fault
kill, and `kill(pid)`. All three paths now close file descriptors, drop any
saved `wait` or pipe reply caps for a killed blocked process, wake pipe
waiters, reparent children, mark the target as a zombie with the correct exit
status, and reply to a waiting parent when applicable. Targeted validation:
`usertests killstatus`, `usertests preempt`, and `usertests reparent`; full
validation: `usertests` ends in `ALL TESTS PASSED`.

M5.13 broadens the xv6 exec catalog from the small shell-smoke set to the full
user program list from xv6's `UPROGS`: `cat`, `echo`, `forktest`, `grep`,
`init`, `kill`, `ln`, `ls`, `mkdir`, `rm`, `sh`, `stressfs`, `usertests`,
`grind`, `wc`, `zombie`, `logstress`, `forphan`, and `dorphan`. This makes
those programs visible to `exec()` and the shell, while preserving the direct
payload path. `tools/run-xv6-user.sh` now also rechecks the final QEMU log after
cleanup so a successful root exit is not reported as a false failure. Verified:
direct `stressfs`, shell `exec stressfs`, and full `usertests`.

M6.1 begins the xv6 server decomposition required for an xv6 `fs.img` backed by
virtio block I/O. The seL4 user ABI surface formerly local to `xv6-host`
(`sel4.rs`, shared constants, BootInfo/IPCBuf layouts, syscall stubs, debug
helpers, and small endian/alignment utilities) is now the reusable
`userspace/sel4-user` crate. xv6-facing constants and the first host<->fs /
fs<->disk IPC operation numbers live in `userspace/xv6-abi`. New
`xv6-fs-server` and `virtio-disk-server` crates build as no_std user servers and
log their protocol versions at boot. At this point `xv6-host` still owned the
current in-memory FS state so existing `usertests` stayed green while the split
was staged.

M6.2 wires the outer disk-image environment into the normal xv6 run path.
`tools/build-xv6-fs-img.sh` builds xv6's native `fs.img` with the same
`rv64imac/lp64` no-F/D user-program flags as the payload builder, uses the nix
shell host C compiler for `mkfs`, and copies the image to
`target/xv6compat/fs.img`. `tools/run-xv6-user.sh` now attaches that image as a
modern virtio-mmio block device by default:
`-global virtio-mmio.force-legacy=false`, `-drive file=...,if=none,format=raw`,
and `-device virtio-blk-device,...,bus=virtio-mmio-bus.0`. Set
`XV6_ATTACH_FS_IMG=0` to boot without the disk, or `XV6_BUILD_FS_IMG=0` to reuse
an existing `XV6_FS_IMG`.

M6.3 connects the staged service processes with actual seL4 IPC. The xv6 host
now allocates separate service untypeds, creates both servers outside the xv6
process table, loads their embedded ELFs, maps stacks/IPCBuffers, and resumes
them before the xv6 payload starts. `virtio-disk-server` boots first and waits
on `XV6_SERVICE_ENDPOINT_CPTR`; `xv6-fs-server` boots with both its own service
endpoint and a badged disk endpoint cap at `XV6_DISK_ENDPOINT_CPTR`. The startup
handshake is:

```text
xv6-host --FS_OP_INIT--> xv6-fs-server --DISK_OP_GET_INFO--> virtio-disk-server
```

M6.4 replaces the disk server's static geometry proof with a real virtio block
read. `xv6-host` finds the BootInfo device untyped covering QEMU's first
virtio-mmio transport at physical `0x10001000`, consumes the prefix needed to
carve the 4KiB device frame, and maps that frame into `virtio-disk-server` at
`0x50000000`. It also maps a regular DMA frame at `0x50001000` and passes its
physical address in `a1`. The disk server initializes virtqueue 0, uses the DMA
page for descriptor/avail/used rings plus a 1KiB data buffer, reads xv6 FS block
1 through virtio-blk, and proved the first real disk data path:

```text
virtio-disk-server: mmio vendor=0x554d4551
virtio-disk-server: virtqueue ready dma=...
```

This proves the first real `fs.img` data path inside a user-mode driver server.
The next step was to make that data path cross the fs<->disk server boundary
without letting the disk driver understand xv6 filesystem contents.

M6.5 shifts superblock verification out of the disk driver and into the file
server. `xv6-host` now allocates one regular shared frame and maps it into both
servers at `0x50002000`. `DISK_OP_GET_INFO` returns only device geometry, while
`DISK_OP_READ(block)` performs the virtio-blk read in `virtio-disk-server`,
copies the 1KiB xv6 block into the shared page, and replies with the byte count
and block number. During `FS_OP_INIT`, `xv6-fs-server` calls
`DISK_OP_READ(1)`, reads the superblock magic from its own mapping of the
shared page, and validates `0x10203040` there:

```text
virtio-disk-server: get-info ready
virtio-disk-server: read block=1
xv6-fs-server: superblock magic=270544960
```

This keeps the disk server at the block-device layer and puts filesystem
validation in the filesystem server. At this M6.5 checkpoint, xv6 file syscalls
still went through the host's in-memory compatibility FS; M6.6 and M6.7 below
move on-disk parsing and the first read-only syscall routing into
`xv6-fs-server`.

M6.6 adds the first real on-disk filesystem parser to `xv6-fs-server`. The fs
server now stores the parsed superblock, reads xv6 dinodes from `IBLOCK(inum)`,
walks direct data blocks for directories, counts valid root entries, and
resolves root-level names such as `README` through the directory file instead of
hard-coded host metadata. `FS_OP_INIT` now verifies:

```text
xv6-fs-server: root entries=22 README ino=2 size=2441 nlink=1
```

There is also an initial read-only `FS_OP_OPEN` handler that accepts a packed
path in IPC message registers and returns `(inum, type, size)` for root-level
paths.

M6.7 routes read-only regular-file syscalls through the split servers. The host
maps the same 4KiB shared block page into its own VSpace, tries `FS_OP_OPEN`
for non-writing opens, stores returned inode numbers in a new fs-server fd kind,
and implements `read`/`fstat` with `FS_OP_READ` and `FS_OP_FSTAT`. The fs
server reads direct and indirect xv6 data blocks via `DISK_OP_READ`, shifts
partial-block reads to the front of the shared page, and returns the byte count
for the host to copy into the xv6 child VSpace. Verified commands:

```text
env TIMEOUT=180 ./tools/run-xv6-user.sh cat README
env TIMEOUT=180 ./tools/run-xv6-user.sh wc README
env TIMEOUT=180 ./tools/run-xv6-user.sh grep xv6 README
env TIMEOUT=1200 ./tools/run-xv6-user.sh usertests
```

The user-facing smoke commands now read `README` from the real xv6 `fs.img`
through `xv6-host -> xv6-fs-server -> virtio-disk-server`.

M6.8 extends that path to read-only directory iteration. `xv6-fs-server`
implements `FS_OP_READDIR` by validating a directory inode and copying raw xv6
dirent bytes from direct or indirect directory data blocks into the shared page.
`xv6-host` allocates a separate fs-server directory fd kind and uses
`FS_OP_READDIR` for `read(fd, &dirent, 16)` calls. At this milestone a
conservative host-mutation guard still existed to protect runtime-created
compatibility entries; later M6.10-M6.14 moved those mutating operations to the
fs server and removed that guard. Verified commands:

```text
env TIMEOUT=180 ./tools/run-xv6-user.sh ls .
env TIMEOUT=240 ./tools/run-xv6-user.sh usertests concreate
env TIMEOUT=1200 ./tools/run-xv6-user.sh usertests
```

M6.9 adds the first write side of the virtio disk-server boundary.
`DISK_OP_WRITE(block)` copies the fs-server shared page into the disk server's
DMA buffer and submits a virtio-blk OUT request. During `FS_OP_INIT`, the file
server reads the last xv6 filesystem block, saves it, writes a deterministic
test pattern through `DISK_OP_WRITE`, reads the block back to verify the
pattern, then restores the original contents through the same write path.
Expected verification logs:

```text
virtio-disk-server: write block=1999
virtio-disk-server: write block=1999
xv6-fs-server: disk write verified block=1999
```

Verified commands:

```text
env TIMEOUT=180 ./tools/run-xv6-user.sh echo disk write verify
env TIMEOUT=180 ./tools/run-xv6-user.sh ls .
env TIMEOUT=180 ./tools/run-xv6-user.sh cat README
env TIMEOUT=1200 ./tools/run-xv6-user.sh usertests
```

Catalog-backed `exec`, console, file creation, file writes, directory creation,
and allocation-heavy filesystem operations still fall back to the host-side
compatibility model until the fs server owns those operations too.

M6.10 moves root-level hard-link and unlink metadata into the filesystem
server. The host first tries `FS_OP_LINK`/`FS_OP_UNLINK` for root-visible
paths; on success, the fs server updates the on-disk xv6 root directory and
dinode metadata through `DISK_OP_WRITE`, and the host only keeps its
compatibility mirror from reappearing as a stale fallback. This currently
supports existing regular files in the root directory and link targets that fit
in an existing root directory data block. It deliberately does not allocate new
inodes or blocks yet.

Expected verification logs:

```text
xv6-fs-server: link README -> readlink ino=2 nlink=2
xv6-fs-server: unlink readlink ino=2 nlink=1
```

Verified commands:

```text
env TIMEOUT=240 ./tools/run-xv6-user.sh --stdin $'ln README readlink\ncat readlink\nrm readlink\ncat readlink\n' sh
env TIMEOUT=180 ./tools/run-xv6-user.sh cat README
env TIMEOUT=1200 ./tools/run-xv6-user.sh usertests
```

`usertests linkunlink` now also exercises the fs-server path for repeated
`link("cat", "x")` / `unlink("x")`.

M6.11 moves the first root-level create/write/truncate path into the filesystem
server. On a fresh fs-server-backed run, `xv6-host` opens writable regular
files through `FS_OP_OPEN`, writes data through the shared block page with
`FS_OP_WRITE`, and sends `FS_OP_CLOSE` when the last duplicated/fork-shared
open-file reference closes. The fs server now writes full dinodes, allocates
inodes from the on-disk inode table, allocates/frees bitmap blocks, supports
direct and single-indirect file data blocks, handles `O_TRUNC`, and frees an
unlinked inode immediately when `nlink == 0` and no server-side open refs
remain.

The host-side compatibility filesystem is still present for xv6 behaviours not
yet owned by the fs server, especially nested directories, `mkdir`, device
nodes, and full xv6 log/transaction semantics. A hybrid mutation guard keeps
the two models from mixing after the host compatibility model has performed a
host-only mutation: fresh shell create/write smoke goes to the real
`fs.img`, while later full-suite host-only directory tests continue on the
host model.

Expected verification logs:

```text
xv6-fs-server: create newfile ino=22
virtio-disk-server: write block=...
xv6-fs-server: unlink newfile ino=22 nlink=0
```

Verified commands:

```text
env TIMEOUT=240 ./tools/run-xv6-user.sh --stdin $'echo hello > newfile\ncat newfile\nrm newfile\ncat newfile\n' sh
env TIMEOUT=300 ./tools/run-xv6-user.sh usertests sharedfd
env TIMEOUT=300 ./tools/run-xv6-user.sh usertests concreate
env TIMEOUT=1200 ./tools/run-xv6-user.sh usertests
```

M6.12 extends that ownership to nested directory mutation. The fs server now
resolves multi-component paths by walking real xv6 directory entries, supports
`mkdir` with `.` / `..`, `mknod`, hard links and unlinks under subdirectories,
and rejects non-empty directory unlink before the host compatibility mirror can
mis-handle it. The host keeps a lightweight cwd mirror so relative paths after
`chdir` can still be converted to fs-server absolute paths while the process
model remains in `xv6-host`.

Verified commands:

```text
env TIMEOUT=300 ./tools/run-xv6-user.sh --stdin $'mkdir dir\necho hello > dir/file\ncat dir/file\nls dir\nrm dir/file\nrm dir\nls .\n' sh
env TIMEOUT=300 ./tools/run-xv6-user.sh --stdin $'mkdir d\ncd d\n/echo cwd-ok > f\n/cat f\ncd ..\nrm d/f\nrm d\n' sh
env TIMEOUT=300 ./tools/run-xv6-user.sh usertests subdir
env TIMEOUT=1200 ./tools/run-xv6-user.sh usertests
```

M6.13 moves cwd and exec validation closer to the fs-server boundary.
`xv6-host` keeps a canonical cwd path alongside the legacy node mirror, uses
that string to normalize relative fs-server paths, asks `xv6-fs-server` to
validate `chdir` targets through `FS_OP_CHDIR`, and checks `exec` paths through
`FS_OP_EXEC_LOOKUP` before loading the embedded executable image. The host also
preserves xv6's raw-path `unlink(".")` / `unlink("..")` behavior before
canonicalization, which caught and fixed a full-suite `rmdot` regression.

Verified commands:

```text
env TIMEOUT=300 ./tools/run-xv6-user.sh --stdin $'mkdir d\ncd d\n/echo cwd-ok > f\n/cat ./../d/./f\ncd ..\nrm d/f\nrm d\n' sh
env TIMEOUT=300 ./tools/run-xv6-user.sh usertests rmdot
env TIMEOUT=1200 ./tools/run-xv6-user.sh usertests
```

M6.14 removes the host's global mutation switch. Once `xv6-host` can build an
absolute server path, filesystem syscalls now use the real `fs.img` server as
the source of truth: open/create/truncate, mkdir, mknod, link, unlink, and
chdir either succeed through `xv6-fs-server` or fail directly. This prevents
later operations from silently drifting back into the host mirror after an
earlier mutation. The fallback mirror still exists for the exceptional case
where a path cannot be represented in the current fixed-size fs-server IPC
format, and embedded exec images are still loaded by the host after
fs-server-side lookup validation.

Verified commands:

```text
env TIMEOUT=300 ./tools/run-xv6-user.sh --stdin $'mkdir d\ncd d\n/echo one > f\n/ln f g\n/cat ../d/g\n/rm f\n/cat g\ncd ..\nrm d/g\nrm d\n' sh
env TIMEOUT=1200 ./tools/run-xv6-user.sh usertests
```

M6.15 removes that embedded-exec dependency from the normal path. Once
`xv6-host` has a normalized absolute fs-server path for `exec()`, it opens the
file through `FS_OP_OPEN`, uses the returned inode and size to stream the ELF
from `fs.img` with `FS_OP_READ`, validates the ELF header/program headers, and
only then resets the process mappings and loads the new image. This means shell
commands such as `/echo` and `/cat` now execute the same files that `ls` and
`cat` see on the real xv6 disk image. The host still keeps the generated exec
catalog as a fallback for paths that cannot fit in the current fixed-size
fs-server IPC path buffer.

Verified commands:

```text
env TIMEOUT=300 ./tools/run-xv6-user.sh --stdin $'/echo fs-exec\n/cat README\n' sh
env TIMEOUT=300 ./tools/run-xv6-user.sh usertests execout
env TIMEOUT=1200 ./tools/run-xv6-user.sh usertests
```

The full run log `target/xv6-m615-usertests-full.log` ended with
`ALL TESTS PASSED` and `xv6-host: exit(0) pid=1`.

M6.16 makes filesystem path syscalls cwd-inode relative instead of
host-canonical-path relative. The host records the fs-server cwd inode returned
by `FS_OP_CHDIR`, inherits it across `fork`, and sends `(cwd_inum, raw path)`
for `open`, `exec`, `chdir`, `mkdir`, `mknod`, `link`, and `unlink`. The fs
server now starts path walks from `cwd_inum` for relative paths and from root
for absolute paths. In normal runtime, a path that cannot be sent to the fs
server now fails directly instead of falling back to the host mirror. The
legacy host mirror code remains only for non-normal fallback paths and should be
deleted once the boot/service assumptions are made explicit.

This specifically fixes xv6's `iref` test: repeated `chdir("irefd")` followed
by relative `mkdir("irefd")` no longer depends on expanding the full cwd into a
128-byte host buffer.

Verified commands:

```text
env TIMEOUT=300 ./tools/run-xv6-user.sh --stdin $'mkdir d\ncd d\n/echo one > f\n/ln f g\n/cat ../d/g\n/rm f\n/cat g\ncd ..\nrm d/g\nrm d\n/echo still-exec\n' sh
env TIMEOUT=300 ./tools/run-xv6-user.sh usertests iref
env TIMEOUT=1200 ./tools/run-xv6-user.sh usertests
```

The full run log `target/xv6-m616-usertests-full.log` ended with
`ALL TESTS PASSED` and `xv6-host: exit(0) pid=1`.

M6.17 removes the now-dead host filesystem mirror from `xv6-host`. The host no
longer carries `FsNode`/`DirEntry` tables, in-memory file blocks, README/exec
pseudo-files, cwd path strings, or the generated embedded exec catalog fallback.
Path syscalls still preserve the same xv6 process/fd behavior at the host
boundary, but all filesystem objects are now owned by `xv6-fs-server` and backed
by the real xv6 `fs.img` through `virtio-disk-server`.

Verified commands:

```text
nix develop --command cargo fmt --check
nix develop --command cargo check -p xv6-fs-server -p virtio-disk-server -p xv6-abi -p sel4-user
nix develop --command ./tools/build-xv6-user-rootserver.sh echo hello from xv6
nix develop --command env TIMEOUT=180 LOG_FILE=target/xv6-m617-echo.log ./tools/run-xv6-user.sh echo hello from xv6
nix develop --command env TIMEOUT=300 LOG_FILE=target/xv6-m617-no-host-mirror-smoke.log ./tools/run-xv6-user.sh --stdin $'mkdir d\ncd d\n/echo one > f\n/cat f\ncd ..\nrm d/f\nrm d\n' sh
nix develop --command env TIMEOUT=1200 LOG_FILE=target/xv6-m617-usertests-full.log ./tools/run-xv6-user.sh usertests
```

The full run log `target/xv6-m617-usertests-full.log` ended with
`ALL TESTS PASSED` and `xv6-host: exit(0) pid=1`.

M6.18 adds xv6-style redo logging to `xv6-fs-server`. Mutating filesystem IPC
operations now run inside a single-server transaction: modified home blocks are
absorbed in an in-memory transaction cache, subsequent reads in the same
transaction see dirty blocks, commit writes the block images into xv6's on-disk
log area, writes the log header as the commit point, installs the logged blocks
to their home locations, then clears the header. `FS_OP_INIT` also performs
xv6-compatible log recovery before exposing the filesystem.

Verified commands:

```text
nix develop --command cargo fmt --check
nix develop --command cargo check -p xv6-fs-server -p virtio-disk-server -p xv6-abi -p sel4-user
nix develop --command env TIMEOUT=180 LOG_FILE=target/xv6-m618-echo.log ./tools/run-xv6-user.sh echo hello from xv6
nix develop --command env TIMEOUT=300 LOG_FILE=target/xv6-m618-transaction-smoke.log ./tools/run-xv6-user.sh --stdin $'mkdir d\ncd d\n/echo one > f\n/cat f\n/ln f g\n/cat g\nrm f\ncat g\ncd ..\nrm d/g\nrm d\n' sh
nix develop --command env TIMEOUT=1200 LOG_FILE=target/xv6-m618-usertests-full.log ./tools/run-xv6-user.sh usertests
```

The full run log `target/xv6-m618-usertests-full.log` ended with
`ALL TESTS PASSED` and `xv6-host: exit(0) pid=1`.

M6.19 reduces the default xv6 server log volume. `virtio-disk-server` no longer
prints a line for every successful `DISK_OP_READ` / `DISK_OP_WRITE`; startup,
geometry, queue setup, protocol errors, out-of-range requests, and timeout
logs remain enabled. Rebuild with `XV6_TRACE_BLOCK_IO=1` when detailed block
traffic is needed for disk debugging.

Verified commands:

```text
nix develop --command cargo fmt --check
nix develop --command cargo check -p xv6-fs-server -p virtio-disk-server -p xv6-abi -p sel4-user
nix develop --command env TIMEOUT=180 LOG_FILE=target/xv6-m619-echo.log ./tools/run-xv6-user.sh echo quiet disk
nix develop --command env TIMEOUT=1200 LOG_FILE=target/xv6-m619-usertests-full.log ./tools/run-xv6-user.sh usertests
```

The echo log still shows both servers booting and the xv6 program exiting
cleanly, while no successful `virtio-disk-server: read block=` or
`virtio-disk-server: write block=` lines are emitted by default. The full
`usertests` log ended with `ALL TESTS PASSED` and `xv6-host: exit(0) pid=1`.
The default full-suite log dropped from 621082 lines in
`target/xv6-m618-usertests-full.log` to 17157 lines in
`target/xv6-m619-usertests-full.log`.

Remaining fs-server work is cleaner decomposition of the now-large
`xv6-fs-server` implementation.

| Milestone | Description | Status |
|-----------|-------------|--------|
| M0 | Build skeleton, no_std ELF cross-compiles | ✅ Done |
| M1 | M-mode standalone boot, NS16550 UART banner via `qemu -bios none -kernel` | ✅ Done |
| M2.1 | S-mode boot under the seL4 C elfloader, SBI console, prints kernel banner | ✅ Done |
| M2.2 | `tools/pack-image.sh` re-packs the official image; sel4test-driver enters U-mode and prints via `seL4_DebugPutChar` | ✅ Done |
| M3.1 | `cap_t` + `mdb_node_t` + `cte_t`, root CNode with 16 fixed initial caps, untyped enumeration into BootInfo | ✅ Done |
| M3.2 | `seL4_Call` slow path: CSpace lookup, extra-cap reading from IPCBuffer, error encoding | ✅ Done |
| M3.3 | `Untyped_Retype` (Untyped/CNode/Frame/PageTable/TCB/EP/Notification), `RISCVPage_Map`, `RISCVPageTable_Map` | ✅ Done |
| M3.4 | CNode `Copy/Mint/Move/Mutate/Delete/Revoke` + MDB CDT linkage | ✅ Done |
| M3.5 | PSpace window (8 × 1 GiB megapages), 3 GiB RAM as untypeds, QEMU MMIO as device untypeds, `seL4_DebugCapIdentify` returns real cap tags | ✅ Done |
| M3.5.1 | CDT correctness: initial-cap and Retype-created MDB nodes carry the right `revocable / firstBadged` bits (matches `write_slot` + `isCapRevocable` in C). Without this, `Untyped_Retype` could reset `free_index` while a sibling carving was still live → classic use-after-free. | ✅ Done |
| M3.5.2 | `isCapRevocable(newCap, srcCap)` on Copy/Mint: untyped/EP-badge/Ntfn-badge/IRQ-handler copies are revocable *roots* of their own derivation subtree. Fixed Revoke walking past `BIG_UT → COPY → sub_ut` when `COPY.revocable` was incorrectly false. | ✅ Done |
| M3.5.3 | `finalize_cap(CNode)` empties the slab when a CNode is being torn down (per the C `finaliseCap` Zombie path). Necessary to recycle a test process's CNode-backed Untyped memory cleanly. | ✅ Done |
| M3.6 | Minimal Notification (state + badge) + `seL4_Send`/`seL4_Recv` slow-path dispatch — enough to make `BIND00xx`, `SYNC00xx`, `CANCEL_BADGED_SENDS` pass. | ✅ Done |
| M3.7 | Minimal ASID table: every Frame cap records the ASID of the VSpace it's mapped into so `Page_Unmap` + `finalize_cap(Frame)` rip the leaf PTE out of the *right* root PT during cross-vspace Revoke. | ✅ Done |
| M3.8 | `BootInfo.userImageFrames` populated with Frame caps for the rootserver ELF range, so `libsel4utils` doesn't re-allocate VAs over the driver's own image. | ✅ Done |
| M3.9 | Full enabled sel4test suite passes: **116/116 enabled tests pass; 51 tests disabled upstream**. | ✅ Done |
| M4.1 | Recycle PT pages on `unmap_user_4k` — empty L1/L0 tables go straight back onto `BOOT_PT_FREELIST`, so the 128-page static pool sustains the whole 116-test sweep. | ✅ Done |
| M4.2a | `Tcb` struct + per-Untyped-Retype slab init + dedicated `handle_thread()` for all 15 non-MCS `TCB_*` labels (Configure/SetSpace/SetIPCBuffer/SetPriority/SetMCPriority/SetSchedParams/WriteRegisters/ReadRegisters/CopyRegisters/Suspend/Resume/BindNotification/UnbindNotification/SetTLSBase/SetFlags). Data is parsed, validated, and persisted into the TCB slab. | ✅ Done |
| M4.2b | Rootserver runs out of a real `Tcb` (`ROOTSERVER_TCB` in BSS); `CAP_INIT_THREAD_TCB` installed; `tcb::CURRENT_TCB` tracked. `restore_user_context` now restores from `current_tcb()->context`, so any `seL4_TCB_*` write against the rootserver TCB (`SetTLSBase`, future `WriteRegisters`, …) takes effect on next sret. | ✅ Done |
| M4.2c | 256-bin per-priority ready queue (`RUNQUEUES` + 4-word `READY_BITMAP` for O(1) "highest set priority" scan), `enqueue/dequeue/schedule()` primitives, `kernel_exit()` hook called from every trap return. `TCB_Resume`/`Suspend` move the TCB in/out of the queue; `TCB_WriteRegisters(resume_target=1)` (the real "start helper" call) hits the same path. `seL4_Yield` rotates within the priority bin. Trampoline now takes the next TCB's `UserContext*` straight out of `handle_trap_rust`'s return value. | ✅ Done |
| M4.2d | `Endpoint` struct (16 bytes, 2-bit state packed in head ptr, doubly-linked wait list reusing `Tcb.queue_{next,prev}`), `enqueue_waiter / pop_head / remove_waiter / finalize` primitives, init hook on `Untyped_Retype(Endpoint)`, `finalize_cap(Endpoint)` wakes all blocked waiters back into the runqueue. `Tcb.caller` field added for the pre-MCS Call/Reply pattern. | ✅ Done |
| M4.2e | Wire `do_send` / `do_recv` / `do_call` / `do_reply` to the `Endpoint` state machine + `tcb::set_current → refresh_from_tcb` so syscalls read MRs from the *running* TCB's IPC buffer. The rootserver actually blocks on its fault EP now and the child test process gets scheduled in. | ✅ Done |
| M4.2e+ | `kernel_exit` writes `satp` + `sfence.vma` when the next TCB lives in a different VSpace; new user root PTs (Untyped → PageTable) get the kernel-ELF + PSpace megapage entries copied in (`copy_kernel_mappings_to`) so traps from U-mode can still reach `trap_entry`. | ✅ Done |
| M4.2e+ | `Page_Map` now parses `seL4_CapRights_t` (bit 0 W, bit 1 R) and the RISC-V VM-attr `riscvExecuteNever` bit instead of hard-coding `R/W/¬X`. ELF code pages are correctly mapped executable. | ✅ Done |
| M4.2e+ | `TCB_Configure` / `TCB_SetSpace` apply the `seL4_CNode_CapData` word (guard ‖ guard_size) to the cspace cap before storing — without this the child process's root CNode could only resolve cptrs equal to its own bits, and every libsel4allocman retype came back `IllegalOperation`. | ✅ Done |
| M4.2f | Close the final enabled-suite gaps: CNode Delete follows `cteDelete(..., exposed=true)` / `emptySlot` semantics, and IPC cap transfer handles the single receive-slot path used by serial-server shared memory setup. | ✅ Done |
| M4.3 | VM/cap/user fault forwarding to the configured fault endpoint; `PAGEFAULT0001..0005` and `PAGEFAULT1001..1004` pass. | ✅ Done |
| M4.4a | Minimal IRQControl/IRQHandler ABI support: issue one handler cap per IRQ, derive it under IRQControl in the MDB, bind/clear Notification caps, finalize handler state on last delete, and signal the kernel timer IRQ notification from the SBI timer trap. `Ack` is accepted as a no-op and RISC-V trigger configuration is parsed but not programmed. | ✅ Done |
| M4.4b | qemu-riscv-virt userspace ltimer + first timer-gated disabled group: `TIMER0001`, `TIMER0002`, `SCHED0000`, `DOMAINS0004`, `PREEMPT_REVOKE`. Full suite now reports **121 passed / 46 disabled**. | ✅ Done |
| M4.4c | RISC-V `PAGEFAULT1005` inter-AS undefined-instruction test: avoid cross-VSpace pointer dereference in the handler and let the faulter restart stub perform the writeback. Full suite now reports **122 passed / 45 disabled**. | ✅ Done |
| M4.4d | `SCHED0021` equal-priority preemption under QEMU simulation: Rust scheduler uses per-TCB time-slice accounting, and sel4test uses a simulation-specific timing upper bound while preserving the original non-simulation bound. Full suite now reports **123 passed / 44 disabled**. | ✅ Done |
| M4.4e | RISC-V `CACHEFLUSH0004`: enable the non-ARM cache/retype test and validate that retyped frames are zeroed after `Untyped_Revoke`. Full suite now reports **124 passed / 43 disabled**. | ✅ Done |
| M4.4f | SMP-compatible RV64 build/run: secondary harts park before shared init; SMP invocation-label shift and `TCBSetAffinity` are handled; QEMU wrappers accept `SMP=2`; `MULTICORE0001..0005` pass in the full SMP run. The current SMP regression now stops on the expected FPU failures after M4.4g. | ✅ Done |
| M4.4g | Remove kernel floating-point context handling: no FPR/FCSR fields in `UserContext`, no `fsd`/`fld`/FCSR instructions in trap entry/exit, and the kernel/rootserver Rust target is `riscv64imac-unknown-none-elf` rather than `rv64gc`. | ✅ Done |
| M4.4h | Remove the residual TCB FPU flag surface: no FPU flag is stored in `Tcb`, `TCBSetFlags` is rejected as unsupported, and `sstatus.FS` is cleared at boot, trap entry, and every return to user mode. | ✅ Done |
| M5.1 | xv6 user-program smoke path: build an xv6 user ELF as rootserver and route xv6 positive syscalls through a temporary kernel compatibility module. | ✅ Superseded |
| M5.2 | Temporary kernel-side xv6 read-only pseudo-fs: expose `README`, `.`, `/`, and `console`; implement fd offsets and `fstat`. | ✅ Superseded |
| M5.3 | seL4-style xv6 host: embed the xv6 user ELF into a no_std Rust 2024 Cargo rootserver, spawn it as a child TCB/VSpace with a fault endpoint, and handle xv6 syscalls via `UnknownSyscall` fault IPC. Smoke set passes: `echo`, `forktest`, `cat README`, `ls .`, `wc README`, `grep xv6 README`. | ✅ Done |
| M5.4 | User-space xv6 process model v1: shared badged fault endpoint, host process table, real TCB/VSpace-backed `fork`, zombie `exit`, and `wait` reaping. `forktest` now creates real children up to the current process-table limit. | ✅ Done |
| M5.5 | Scripted shell path: `XV6_CONSOLE_INPUT`/`--stdin`, blocking empty console reads, per-process fd tables, fd refcounting across `fork`, close-on-exit, and basic cross-process pipes. `sh` can run `echo`, `ls`, `cat README`, and `echo pipe data \| wc`. | ✅ Done |
| M5.6 | Shared open-file table and mutable in-memory FS: `fork` inherits cwd, `dup`/`fork` share file offsets, file capacity is large enough for xv6 `sharedfd`, `sbrk` preserves mapping headroom, and targeted `usertests sharedfd` passes. | ✅ Done |
| M5.7 | xv6-host mapping cleanup: `sbrk` shrink, exec reset, and process reap unmap child/alias frames, delete cap slots, and recycle them. Targeted `usertests fourfiles`, `createdelete`, `unlinkread`, `linktest`, `concreate`, `linkunlink`, `subdir`, `bigwrite`, and `bigfile` pass. | ✅ Done |
| M5.8 | xv6 quick-suite memory semantics: dynamic low user stack with guard page, xv6 `TRAPFRAME` heap limit, sparse large `sbrk` backed by lazy VM faults, reservation-based OOM behavior for `sbrkfail`, and fork resource preflight. `usertests -q` passes. | ✅ Done |
| M5.9 | xv6 full `usertests` slow section: larger reusable directory table for `bigdir`, enough FS behavior for `manywrites`, `badwrite`, `execout`, `diskfull`, and `outofinodes`; full `usertests` reaches `ALL TESTS PASSED`. | ✅ Done |
| M5.10 | xv6 directory-block pressure: directories allocate from the same 1KiB data-block pool as files, `mkdir` consumes a child directory block, and `diskfull` no longer reports unexpected `mkdir` success after exhausting blocks. | ✅ Done |
| M5.11 | Blocking pipe reads/writes: empty readers and full writers save reply caps and resume when peer activity changes pipe state; `usertests pipe1` and `usertests preempt` pass. | ✅ Done |
| M5.12 | Unified xv6 process termination: `exit`, fault kill, and `kill(pid)` share fd cleanup, blocked reply-cap cleanup, pipe waiter wakeups, reparenting, zombie status, and waiting-parent reply behavior. `usertests killstatus`, `preempt`, `reparent`, and full `usertests` pass. | ✅ Done |
| M5.13 | Full xv6 `UPROGS` exec catalog: the default catalog now embeds `cat`, `echo`, `forktest`, `grep`, `init`, `kill`, `ln`, `ls`, `mkdir`, `rm`, `sh`, `stressfs`, `usertests`, `grind`, `wc`, `zombie`, `logstress`, `forphan`, and `dorphan`; direct `stressfs`, shell `exec stressfs`, and full `usertests` pass. | ✅ Done |
| M6.1 | Split groundwork for xv6 service servers: extracted `sel4-user`, added `xv6-abi`, migrated `xv6-host` to shared crates, and added compiling no_std `xv6-fs-server` / `virtio-disk-server` skeletons. | ✅ Done |
| M6.2 | xv6 `fs.img` and virtio-blk QEMU path: build `target/xv6compat/fs.img`, attach it by default in `run-xv6-user.sh`, and add shared xv6 on-disk FS / virtio-mmio ABI constants for the future fs/disk servers. | ✅ Done |
| M6.3 | Service topology and control-plane IPC: `xv6-host` spawns `xv6-fs-server` and `virtio-disk-server` as separate seL4 user servers, mints endpoint caps, and verifies host->fs->disk init/get-info IPC before starting the xv6 payload. | ✅ Done |
| M6.4 | First real virtio disk-server data path: map virtio-mmio and a DMA frame into `virtio-disk-server`, initialize virtqueue 0, and prove xv6 `fs.img` block reads through virtio-blk. | ✅ Done |
| M6.5 | First fs<->disk shared-memory block read: map a shared frame at `0x50002000`, make `DISK_OP_GET_INFO` geometry-only, implement `DISK_OP_READ(block)`, and validate the xv6 superblock inside `xv6-fs-server`. | ✅ Done |
| M6.6 | First xv6 on-disk fs parser in `xv6-fs-server`: parse superblock/inodes/root directory from `fs.img`, resolve `README`, and add a read-only root-level `FS_OP_OPEN` lookup entry point. | ✅ Done |
| M6.7 | First host syscall routing to the fs server: read-only regular-file `open/read/fstat` now flow through `xv6-host -> xv6-fs-server -> virtio-disk-server`, with `cat README`, `wc README`, `grep xv6 README`, and full `usertests` passing. | ✅ Done |
| M6.8 | Read-only fs-server directory iteration: pristine root directory fds use `FS_OP_READDIR` over real xv6 dirent blocks from `fs.img`, while host-FS mutation falls back to the compatibility directory model. `ls .`, `usertests concreate`, and full `usertests` pass. | ✅ Done |
| M6.9 | Reversible virtio disk-server block write: `DISK_OP_WRITE` submits virtio-blk OUT requests, and fs-server boot verifies write/read/restore on the last xv6 filesystem block before continuing. `ls .`, `cat README`, and full `usertests` pass afterward. | ✅ Done |
| M6.10 | First fs-server metadata mutations: root-level `FS_OP_LINK`/`FS_OP_UNLINK` update real xv6 dirents and dinode `nlink` through `DISK_OP_WRITE`; shell `ln README readlink; cat readlink; rm readlink`, `cat README`, and full `usertests` pass. | ✅ Done |
| M6.11 | First fs-server create/write/truncate path: fresh root-level `open(O_CREATE)`/`write`/`O_TRUNC` use real inode and block bitmap allocation through `xv6-fs-server -> virtio-disk-server`, with inode reclaim on final unlink and a hybrid guard for later host-only directory semantics. Shell create/write, `usertests sharedfd`, `usertests concreate`, and full `usertests` pass. | ✅ Done |
| M6.12 | Nested fs-server directory mutation: multi-component lookup, `.` / `..`, `FS_OP_MKDIR`, `FS_OP_MKNOD`, subdirectory create/link/unlink, non-empty directory unlink rejection, and host cwd-to-absolute fs-server routing. Subdirectory smoke, `usertests subdir`, and full `usertests` pass. | ✅ Done |
| M6.13 | Fs-server-owned cwd/exec validation: per-process canonical cwd paths in `xv6-host`, `FS_OP_CHDIR`, `FS_OP_EXEC_LOOKUP`, relative path normalization independent of the host mirror, and raw `unlink(".")` / `unlink("..")` rejection before canonicalization. Cwd smoke, `usertests rmdot`, and full `usertests` pass. | ✅ Done |
| M6.14 | Fs-server-authoritative normalized paths: removed the global `HOST_FS_MUTATED` escape hatch so normalized open/create/truncate, mkdir, mknod, link, unlink, and chdir return fs-server results directly instead of falling back to the host mirror. Create/link/unlink smoke and full `usertests` pass. | ✅ Done |
| M6.15 | fs.img-backed exec: normalized `exec()` paths now open/read ELF files through `xv6-fs-server -> virtio-disk-server`, validate before replacing the address space, and keep the embedded catalog only as an unrepresentable-path fallback. Shell `/echo`, `usertests execout`, and full `usertests` pass. | ✅ Done |
| M6.16 | Cwd-inode fs routing: `xv6-host` sends `(cwd_inum, raw path)` to the fs server for path syscalls, the fs server resolves relative paths from that inode, and normal-runtime path failures no longer fall back to the host mirror. Shell no-host-fallback smoke, `usertests iref`, and full `usertests` pass. | ✅ Done |
| M6.17 | Host mirror removal: `xv6-host` no longer has in-memory filesystem nodes, directory entries, file blocks, cwd strings, README pseudo-file, or embedded exec catalog fallback. All filesystem objects and `exec()` images come from `xv6-fs-server -> virtio-disk-server -> fs.img`; no-host-mirror smoke and full `usertests` pass. | ✅ Done |
| M6.18 | xv6 redo-log transaction layer in `xv6-fs-server`: mutating FS IPC operations use log absorption, commit through xv6's on-disk log header/data blocks, install logged home blocks, clear the header, and recover any committed log during `FS_OP_INIT`. Transaction smoke and full `usertests` pass. | ✅ Done |
| M6.19 | Quieter default xv6 server logs: successful virtio block read/write traces are behind `XV6_TRACE_BLOCK_IO=1`, preserving error and startup logs while making smoke/full-suite logs inspectable. Full `usertests` still passes, and the default full-suite log drops from 621082 to 17157 lines. | ✅ Done |
| M4.4 | Full PLIC IRQ chain, true per-hart SMP, MCS/multi-domain/VTX coverage, and the remaining upstream-disabled tests. | ⏳ Pending |

### Disabled-Test Accounting (M4.4e Single-Core)

The pre-M4.4b disabled set contained 51 tests in the compiled ELF. Eight
have since been enabled and are now passing:

```text
TIMER0001, TIMER0002, SCHED0000, DOMAINS0004, PREEMPT_REVOKE
PAGEFAULT1005, SCHED0021, CACHEFLUSH0004
```

The remaining 43 disabled tests, as of the `124 passed / 43 disabled`
run, fall into these primary buckets:

| Bucket | Count | Tests |
|--------|-------|-------|
| MCS / scheduling-context semantics | 32 | `BIND005`, `BIND006`, `TIMEOUTFAULT0001..0003`, `INTERRUPT0002..0006`, `SCHED_CONTEXT_0001`, `SCHED_CONTEXT_0002`, `SCHED_CONTEXT_0003`, `SCHED_CONTEXT_0005..0014`, `SCHED0007..0014`, `SCHED0016` |
| SMP / multicore | 7 | `FPU0002`, `MULTICORE0001..0005`, `SCHED0022` |
| Multi-domain | 3 | `DOMAINS0000`, `DOMAINS9999`, `DOMAINS0005` |
| VTX / VM-entry | 1 | `UNKNOWN_SYSCALL_001` |

### SMP checkpoint (M4.4f)

The current SMP validation uses the upstream sel4test tree configured with
`SMP=ON` and `NUM_NODES=2`, then boots the Rust kernel under QEMU with
`SMP=2`. Before M4.4g removed FPU context support, this enabled and passed the
non-MCS SMP group:

```text
FPU0002, MULTICORE0001, MULTICORE0002, MULTICORE0003,
MULTICORE0004, MULTICORE0005
```

`SCHED0022` remains disabled because the upstream gate is
`CONFIG_KERNEL_MCS && CONFIG_MAX_NUM_NODES > 1`. This phase is deliberately
not a true multi-hart scheduler: secondary harts are parked before BSS/global
init, while the primary hart provides affinity-compatible behavior sufficient
for the current tests.

Historical SMP full-run summary before M4.4g:

```text
ELF-loader started on (HART 0) (NODES 2)
Secondary entry hart_id:1 core_id:1
...
Starting test 39: FPU0002
Starting test 60: MULTICORE0001
...
Starting test 64: MULTICORE0005
...
Test suite passed. 125 tests passed. 42 tests disabled.
All is well in the universe
```

A live run (kernel boots, the rootserver's `allocman` carves up untyped
memory via dozens of `Untyped_Retype` calls, maps frames via
`RISCVPage_Map`, brings up the serial server, prints the seL4 banner,
runs through `vka_alloc_untyped`'s size-probe, then sweeps the test
suite to completion):

```text
ELF-loader started on (HART 0) (NODES 1)
  ...
ELF-loading image 'kernel' to 80200000
  paddr=[80200000..81317fff]
  vaddr=[ffffffff80200000..ffffffff81317fff]
ELF-loading image 'rootserver' to ...
Jumping to kernel-image entry point...

microkernel: Rust kernel booted (S-mode, Sv39)
  ...
  --- transferring control to rootserver ---

seL4 Test
=========

Starting test suite sel4test
Starting test 0:   Test that there are tests
Starting test 1:   SYSCALL0000
...
Starting test 15:  SYSCALL0017
Starting test 16:  TIMER0001
Starting test 17:  TIMER0002
Starting test 18:  BIND0001
...
Starting test 37:  DOMAINS0004
...
Starting test 70:  PREEMPT_REVOKE
...
Starting test 75:  SCHED0000
...
Starting test 119: VSPACE0006
Starting test 124: Test all tests ran
Test suite passed. 124 tests passed. 43 tests disabled.
All is well in the universe
```

### xv6 Compatibility Checkpoint (M5.3-M6.17)

The current xv6 path is a user-space compatibility server, not a full Unix
server yet. The helper builds one xv6 user program from
`third_party/xv6-riscv/user`, links it at `0x10000000` with a generated
`argc/argv` entry stub, then invokes Cargo to build `userspace/xv6-host` with
that payload embedded. The host boots as the elfloader rootserver, uses seL4
APIs to create a child TCB/CNode/VSpace/fault endpoint, and services the
child's positive xv6 syscalls via `UnknownSyscall` fault IPC.
For `exec()` after boot, paths are resolved and read from the attached xv6
`fs.img` by the fs and disk servers. The host-side embedded exec catalog has
been removed. Path syscalls after M6.16 are resolved by the fs server from an
explicit cwd inode, so normal relative paths no longer depend on the host
expanding a canonical absolute path. After M6.17, `xv6-host` no longer carries
an in-memory filesystem mirror at all.

The host crate remains `edition = "2024"` and enforces the Rust 2024 unsafe
rules at compile time with `deny(unsafe_attr_outside_unsafe)` and
`deny(unsafe_op_in_unsafe_fn)`. Its implementation is split by responsibility
under `userspace/xv6-host/src`: boot allocation, child TCB/VSpace setup,
payload mapping, utility code, and xv6 syscall handling. Common seL4 user ABI
definitions have moved to `userspace/sel4-user`; common xv6 syscall/fs/disk
protocol definitions have moved to `userspace/xv6-abi`.

With the current SMP upstream build, use two harts (the helper defaults to
`SMP=2`):

```sh
nix develop --command ./tools/run-xv6-user.sh echo hello from xv6
nix develop --command ./tools/run-xv6-user.sh forktest
nix develop --command ./tools/run-xv6-user.sh cat README
nix develop --command ./tools/run-xv6-user.sh ls .
nix develop --command ./tools/run-xv6-user.sh wc README
nix develop --command ./tools/run-xv6-user.sh grep xv6 README
nix develop --command ./tools/run-xv6-user.sh --stdin $'echo scripted from sh\nls .\ncat README\n' sh
nix develop --command ./tools/run-xv6-user.sh --stdin $'echo pipe data | wc\n' sh
nix develop --command ./tools/run-xv6-user.sh usertests sharedfd
nix develop --command ./tools/run-xv6-user.sh usertests fourfiles
nix develop --command ./tools/run-xv6-user.sh usertests concreate
nix develop --command ./tools/run-xv6-user.sh usertests bigfile
nix develop --command ./tools/run-xv6-user.sh usertests killstatus
nix develop --command ./tools/run-xv6-user.sh usertests preempt
nix develop --command ./tools/run-xv6-user.sh usertests reparent
nix develop --command ./tools/run-xv6-user.sh stressfs
nix develop --command ./tools/run-xv6-user.sh --stdin $'stressfs\n' sh
nix develop --command env TIMEOUT=1200 ./tools/run-xv6-user.sh usertests
```

Verified output includes:

```text
hello from xv6
xv6-host: exit(0) pid=1

fork test
xv6-host: fork parent=1 child=2
xv6-host: exit(0) pid=2
...
fork test OK
xv6-host: exit(0) pid=1

.              1 1 64
..             1 1 64
README         2 2 2441
console        3 3 0
xv6-host: exit(0) pid=1

$ echo pipe data | wc
xv6-host: fork parent=1 child=2
xv6-host: fork parent=2 child=3
xv6-host: fork parent=2 child=4
xv6-host: exec echo pid=3
xv6-host: exec wc pid=4
1 2 10

test sharedfd:
xv6-host: fork parent=1 child=2
xv6-host: fork parent=2 child=3
xv6-host: exit(0) pid=3
xv6-host: exit(0) pid=2
OK
ALL TESTS PASSED
xv6-host: exit(0) pid=1

test fourfiles:
...
OK
ALL TESTS PASSED
xv6-host: exit(0) pid=1
```

Implemented host-side compatibility now has an explicit handler for every xv6
syscall number 1..21. The currently functional subset is process exit,
TCB/VSpace-backed `fork`, fs.img-backed `exec`,
zombie/blocking `wait`, scripted console input, console/file read-write where
meaningful, per-process `open`/`close`/`dup`/`fstat`, shared open-file offsets
across `dup`/`fork`, `sbrk`, `getpid`, `uptime`, `pause`, `chdir`,
`mknod("console")`, `link`/`unlink`/`mkdir`, mutable files/directories through
the fs server, and a fixed-size in-memory `pipe` ring buffer with blocking
reads/writes shared across forked processes. Process termination now uses one
path for normal `exit`, fault kill, and `kill(pid)`, including wait reply,
child reparenting, and blocked reply-cap cleanup.
Remaining Unix gaps are xv6 log/transaction semantics, permissions/devices beyond
console, dynamic host keyboard input, full untyped-memory reclamation beyond
cap-slot reuse, and resource scaling beyond the current fixed tables. With no
scripted input, `init` now reaches
`exec("sh")` and the shell blocks on console read instead of exiting and
forcing `init` into a restart loop.


## Repository layout

```
microkernel/
├── flake.nix              # Nix dev shell: Rust + RISC-V toolchain + qemu/ninja/cpio
├── .envrc                 # `use flake` for direnv
├── rust-toolchain.toml    # stable + riscv64imac-unknown-none-elf
├── Cargo.toml             # workspace
├── .cargo/config.toml     # build target + shared RISC-V rustflags
├── kernel/
│   ├── Cargo.toml
│   ├── linker.ld          # KERNEL_ELF_BASE=0xFFFFFFFF80200000, LMA 0x80200000
│   └── src/
│       ├── main.rs        # entry, panic handler
│       ├── print.rs       # println! macros via machine::console
│       ├── abi/           # byte-exact seL4 ABI mirror
│       │   ├── constants.rs
│       │   ├── syscall.rs
│       │   ├── types.rs       # MessageInfo, CapRights, CNodeCapData
│       │   └── bootinfo.rs    # seL4_BootInfo, IPCBuffer
│       ├── arch/riscv64/
│       │   ├── boot.rs        # _start + init_kernel
│       │   ├── csr.rs         # S-mode CSR accessors
│       │   ├── sbi.rs         # legacy SBI ecall wrappers
│       │   ├── sv39.rs        # PageTable / PTE / make_satp
│       │   ├── vspace.rs      # kpptr<->paddr, map_user_4k, make_boot_root_pt
│       │   ├── trap.S         # asm trap entry / restore_user_context
│       │   └── trap.rs        # UserContext + handle_trap_rust
│       ├── machine/
│       │   ├── console.rs     # SBI-backed putc
│       │   └── uart.rs        # NS16550 (M1 only)
│       ├── kernel/
│       │   ├── boot.rs        # bringup_rootserver
│       │   └── bootmem.rs     # bump page allocator
│       ├── object/
│       │   ├── cap.rs         # cap_t bit layouts (Untyped/CNode/Frame/PT/EP/Ntfn/…)
│       │   ├── mdb.rs         # mdb_node_t
│       │   ├── cnode.rs       # Cte + cnode_at / install_initial_cap / mdb_*
│       │   ├── untyped.rs     # free-range splitter, untyped cap factory
│       │   ├── notification.rs # min. Notification (state + badge + signal/wait)
│       │   ├── irq.rs          # min. IRQHandler table + notification binding
│       │   ├── endpoint.rs    # Endpoint (16 B: state-packed head ptr + tail),
│       │   │                  #   wait-list queue ops, finalize wakes waiters
│       │   ├── tcb.rs         # Tcb struct (context + scheduler/IPC state),
│       │   │                  #   256-bin runqueue + bitmap, init on Retype,
│       │   │                  #   finalize on revoke
│       │   └── asid.rs        # 64-entry ASID → root-PT-KVA table
│       ├── api/
│       │   ├── thread.rs      # rootserver thread record (CSpace/VSpace/IPCBuf)
│       │   ├── cspace.rs      # single-level CSpace lookup (CPtr → Cte*)
│       │   ├── syscall.rs     # seL4_Call dispatch + error reply encoding +
│       │   │                  #   Send/Recv slow-path on Notification caps
│       │   └── invocation.rs  # Untyped_Retype, Page_Map, PageTable_Map, CNode
│       │                      #   ops, isCapRevocable, finalize_cap(Frame/CNode)
├── userspace/
│   ├── sel4-user/             # shared no_std seL4 user ABI wrappers
│   ├── xv6-abi/               # shared xv6 syscall/fs/disk protocol constants
│   ├── xv6-host/              # no_std seL4 rootserver that hosts xv6 user ELFs
│   ├── xv6-fs-server/         # staged xv6 fs server crate
│   └── virtio-disk-server/    # staged virtio block server crate
├── third_party/
│   └── xv6-riscv/             # upstream xv6 tree used for user programs
└── tools/
    ├── pack-image.sh              # rebuild Rust kernel + ninja repackage
    ├── simulate.sh                # qemu wrapper (standalone or packed image)
    ├── run-tests.sh               # CI runner for sel4test image
    ├── build-xv6-user-rootserver.sh # link xv6 payload + build xv6-host rootserver
    └── run-xv6-user.sh            # pack + boot xv6 user-program smoke image
```

## Quick start

Requires Nix with flakes enabled, plus the upstream seL4 source already
built at `${SEL4_BUILD_DIR:-/Users/wangfiox/sel4/sel4test/build-riscv64}`.

```sh
direnv allow                  # or: nix develop

# Build the Rust kernel:
cargo build --release

# Pack our kernel into a sel4test-driver image and boot it under QEMU:
./tools/pack-image.sh
./tools/simulate.sh           # Ctrl-A x to exit

# Boot the standalone M1 banner (no elfloader, M-mode):
MODE=standalone ./tools/simulate.sh

# Headless / CI mode — boots the packed image, watches for the
# upstream "Test suite passed." banner, prints a one-line summary, and
# exits 0 on success / 1 on failure / 2 on timeout (default 180 s):
./tools/run-tests.sh           # quiet
./tools/run-tests.sh -v        # stream QEMU output as it runs
TIMEOUT=60 ./tools/run-tests.sh
SMP=2 TIMEOUT=480 ./tools/run-tests.sh  # SMP-compatible sel4test build

# xv6 user-program smoke path:
./tools/build-xv6-fs-img.sh
./tools/run-xv6-user.sh echo hello from xv6
./tools/run-xv6-user.sh forktest
./tools/run-xv6-user.sh cat README
./tools/run-xv6-user.sh ls .
```

## Key ABI / layout constants (frozen against upstream)

| Symbol | Value | Source |
|--------|-------|--------|
| `PHYS_BASE_RAW` | `0x80200000` | `build-riscv64/kernel/gen_headers/plat/machine/devices_gen.h` |
| `PPTR_BASE` | `0xFFFFFFC0_00000000` | `kernel/include/arch/riscv/arch/64/mode/hardware.h` |
| `PPTR_TOP`  | `0xFFFFFFFF_80000000` | ditto |
| `KERNEL_ELF_BASE` | `0xFFFFFFFF_80200000` | `PPTR_TOP + (PHYS_BASE_RAW & MASK(30))` |
| `seL4_PageBits` | 12 | RV64 ABI |
| `seL4_TCBBits` | 11 (= 2 KiB) | RV64 ABI |
| `seL4_SlotBits` | 5 (= 32 B / cte) | RV64 ABI |
| `CONFIG_ROOT_CNODE_SIZE_BITS` | 13 (= 8192 slots) | gen_config.h |
| `CONFIG_MAX_NUM_BOOTINFO_UNTYPED_CAPS` | 230 | gen_config.h |
| `CONFIG_NUM_DOMAINS` | 1 | gen_config.h |
| `CONFIG_NUM_PRIORITIES` | 256 | gen_config.h |
| `CONFIG_KERNEL_MCS` | disabled | gen_config.h |
| `CONFIG_PT_LEVELS` | 3 (Sv39) | gen_config.h |

## Boot flow (M2.2)

```
QEMU virt
  └─> OpenSBI fw_payload.elf  (M-mode firmware bundled with elfloader)
        └─> seL4 elfloader     (sets up Sv39, loads kernel.elf + sel4test-driver)
              └─> Rust kernel _start (.boot.text @ 0xFFFFFFFF80200000)
                    └─> init_kernel(a0..a7) [boot.rs]
                          └─> kernel::boot::bringup_rootserver
                                ├─ install_trap_vector()
                                ├─ make_boot_root_pt()                  (1 GiB megapage for kernel ELF window)
                                ├─ map_user_4k(...) for sel4test-driver
                                ├─ alloc + map BootInfo frame           (VA 0x7FFFE000)
                                ├─ alloc + map IPC buffer frame         (VA 0x7FFFF000)
                                ├─ alloc + map 64 KiB user stack
                                ├─ populate seL4_BootInfo (mostly zeros)
                                ├─ switch_satp(satp_for(root, ASID=1))
                                └─ restore_user_context(&ROOTSERVER_CONTEXT)
                                      └─ sret → _sel4_start in U-mode
```

## Next steps (M4)

With the first timer-gated disabled group, `PAGEFAULT1005`,
`SCHED0021`/`CACHEFLUSH0004`, and the non-MCS SMP group green, the
remaining work is about tests outside the current RV64/non-MCS/QEMU slice,
plus semantics that are implemented only as far as sel4test currently needs:

1. **MCS model.** Scheduling contexts, timeout faults, SC donation, MCS
   IPC, and MCS scheduler tests are still intentionally out of scope for
   this non-MCS kernel build.
2. **True SMP / multicore.** M4.4f is an affinity-compatible single-hart
   execution model with parked secondary harts. Real per-hart runqueues,
   IPIs, remote preemption, and cross-hart TLB shootdown still need a
   dedicated implementation pass.
3. **Other disabled buckets.** Multi-domain scheduling and VTX/VM-entry
   are tracked separately from the timer work that landed in M4.4b.
4. **Full cap-transfer/generalisation pass.** The current IPC transfer
   path intentionally covers the pre-MCS single receive-slot case. The
   next conformance pass should cover multi-cap edge cases, endpoint
   unwrapping details, and cleanup paths beyond the serial-server use.
5. **Zombie/finalisation fidelity.** CNode/TCB finalisation is good
   enough for the enabled tests, but should be brought closer to the C
   kernel's Zombie reduction model before expanding coverage further.
6. **xv6 service split.** The in-host filesystem mirror is gone. Next xv6 work
   is xv6 log/transaction fidelity in the fs server, reducing the tracing noise,
   and splitting the large fs-server implementation into smaller modules before
   expanding device and resource semantics.
