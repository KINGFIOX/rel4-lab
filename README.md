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
  `env TIMEOUT=1200 ./tools/run-xv6-user.sh usertests`. `virtio-disk-server`
  now saves the fs-server caller's reply cap, submits one virtio request, and
  returns to its service endpoint while DMA is in flight. The virtio IRQ
  notification is bound to the disk-server TCB, so completion arrives through
  the normal receive loop; the server then ACKs the MMIO/IRQHandler path,
  consumes the used ring, and replies through the saved reply cap. The disk
  server now has two independent request slots, each with its own descriptor
  chain, DMA data/status storage, and saved reply cap; the fs server currently
  still issues disk RPCs synchronously, but host/fs payload exchange uses shared
  slots 0..2 for xv6-sized writes while fs/disk block I/O uses shared slot 3.
  Regular-file `write()` now uses
  xv6's 3072-byte `filewrite` transaction chunk size and returns `-1` for
  partial inode writes, matching xv6 rather than exposing internal fs-server
  partial counts. The fs server
  also now retains cwd inode references for live processes and reclaims
  post-crash orphaned inodes during log recovery, matching the xv6
  `forphan`/`dorphan` recovery path. `xv6-fs-server` IPC operation handlers
  have been split into `ops.rs`, leaving `main.rs` focused on boot/init and
  request dispatch. `xv6-host` has also started the same host-side
  decomposition by moving pipe ring state and reader/writer accounting into
  `pipes.rs`, and open-file backing state/refcounts/offsets into `files.rs`.
  The xv6 `pause(n)` syscall now blocks on a saved reply cap and wakes from
  the host's timer notification after the requested tick deadline, instead of
  merely yielding once or advancing time on unrelated syscalls. The run helper
  also has an explicit `--expect-timeout` mode for infinite stress programs
  such as `grind`. The disk/fs protocol now also has `DISK_OP_FLUSH`; the
  virtio driver negotiates `VIRTIO_BLK_F_FLUSH` when available and otherwise
  treats flush as a successful no-op. The fs redo log now issues flush barriers
  after log-data writes, after the commit header, after home-block install, and
  after clearing the log header. The disk protocol now accepts an explicit
  shared-buffer slot for read/write requests, and `virtio-disk-server` tracks
  two independent request slots with per-slot DMA request/data/status storage
  and per-slot saved reply caps. The current fs server still issues disk RPCs
  synchronously, but it no longer reuses the host/fs payload slot for disk block
  transfers and now uses an explicit `Send + DISK_OP_COMPLETE` completion id
  path instead of `seL4_Call` for disk data/flush operations. `xv6-host` now
  treats `XV6_EBUSY` from `xv6-fs-server` as a retryable concurrency guard and
  yields/retries rather than surfacing it as an xv6 syscall failure. Disk
  completions no longer depend on a blocking endpoint send back into the fs
  server: `virtio-disk-server` writes completions into a shared ring page and
  signals a bound completion notification, while `xv6-fs-server` drains the
  ring when waiting for the matching completion id. The disk event loop is no
  longer hard-coded to one pending descriptor chain. Before accepting a new disk
  RPC, the driver now also drains already-visible used-ring completions without
  waiting; pending request state is installed before publishing a descriptor head
  to the avail ring, so fast
  DMA/IRQ completion cannot race ahead of request-slot ownership. The fs server
  now also replies `XV6_EBUSY` to unrelated host/fs IPC received while it is
  waiting for a disk completion, preserving the pending disk operation and
  preventing accidental shared-slot or transaction re-entry. The xv6 host
  process/resource limits are now closer to upstream xv6: `MAX_PROCS` is 64,
  `MAX_EXEC_ARGS` is 32, `MAX_PIPES` is 32, and child user frames come from a
  reusable global frame pool that only recycles pool-owned mappings.
  `xv6-fs-server` also has a small clean block cache in front of disk RPCs:
  logged transaction blocks still dominate reads, transaction writes invalidate
  cached home blocks, and successful raw writes refresh the committed cache
  image. This reduces repeated inode/bitmap/directory block reads without
  weakening the xv6 redo-log ordering. The xv6 run tooling now serializes the
  rootserver/fs.img/elfloader pack stage with a reusable build lock and gives
  default QEMU runs a per-run fs.img copy, so parallel smoke runs no longer race
  on xv6's generated `fs.img` or the shared seL4 elfloader staging directory.
  Console reads without `XV6_CONSOLE_INPUT` now block in `xv6-host` on a saved
  reply cap and resume from timer-driven polling of a local debug getchar
  syscall, so the shell can consume runtime QEMU stdin instead of requiring
  build-time scripted input. Child IPC-buffer frames are now treated as
  process-control resources rather than ordinary user mappings, which avoids
  mapping-table pressure and recycled-slot aliasing during repeated fork/reap
  cycles.
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

M6.20 starts decomposing `xv6-fs-server` without changing behavior. Disk/runtime
filesystem types and global state moved to `types.rs`; transaction, redo-log,
disk IPC, and shared-block helpers moved to `block.rs`. This reduced
`main.rs` from 1772 to 1322 lines while keeping the same fs.img-backed service
topology.

Verified commands:

```text
nix develop --command cargo fmt --check
nix develop --command cargo check -p xv6-fs-server -p virtio-disk-server -p xv6-abi -p sel4-user
nix develop --command env TIMEOUT=180 LOG_FILE=target/xv6-m620-echo.log ./tools/run-xv6-user.sh echo split fs server
nix develop --command env TIMEOUT=1200 LOG_FILE=target/xv6-m620-usertests-full.log ./tools/run-xv6-user.sh usertests
```

The full run log `target/xv6-m620-usertests-full.log` ended with
`ALL TESTS PASSED` and `xv6-host: exit(0) pid=1`.

M6.21 continues the same decomposition by moving raw inode IO, open-reference
tracking, inode allocation/free, truncation, data writes, bitmap allocation,
and direct/indirect block mapping into `inode.rs`. `main.rs` now stays focused
on IPC handlers plus directory/path operations and is down to 957 lines.

Verified commands:

```text
nix develop --command cargo fmt --check
nix develop --command cargo check -p xv6-fs-server -p virtio-disk-server -p xv6-abi -p sel4-user
nix develop --command env TIMEOUT=180 LOG_FILE=target/xv6-m621-echo.log ./tools/run-xv6-user.sh echo inode split
nix develop --command env TIMEOUT=1200 LOG_FILE=target/xv6-m621-usertests-full.log ./tools/run-xv6-user.sh usertests
```

The full run log `target/xv6-m621-usertests-full.log` ended with
`ALL TESTS PASSED` and `xv6-host: exit(0) pid=1`.

M6.22 moves directory and path resolution into `dir.rs`: dirent read/write,
root lookup, path and parent lookup, empty-directory checks, directory entry
insertion/removal, node creation, and path-byte logging now live outside the
IPC handler file. `main.rs` is down to 587 lines and is mostly request
dispatch plus per-op argument validation.

Verified commands:

```text
nix develop --command cargo fmt --check
nix develop --command cargo check -p xv6-fs-server -p virtio-disk-server -p xv6-abi -p sel4-user
nix develop --command env TIMEOUT=180 LOG_FILE=target/xv6-m622-echo.log ./tools/run-xv6-user.sh echo dir split
nix develop --command env TIMEOUT=1200 LOG_FILE=target/xv6-m622-usertests-full-rerun.log ./tools/run-xv6-user.sh usertests
```

The full rerun log `target/xv6-m622-usertests-full-rerun.log` ended with
`ALL TESTS PASSED` and `xv6-host: exit(0) pid=1`.

M6.23 makes virtio block completion IRQ-driven instead of disk-server polling.
The kernel now has a minimal QEMU virt PLIC path for supervisor external
interrupts: `IRQControl` enables PLIC sources, external traps claim the pending
source, `IRQHandler_Ack` completes it, and the all-blocked kernel idle loop
also services already-pending external IRQs so a blocked disk server can wake
without returning to a runnable user thread first. `xv6-host` mints a badged
send cap for the virtio IRQ notification and passes a receive cap plus
`IRQHandler` cap to `virtio-disk-server`. The disk server submits one virtqueue
request at a time, does not spin-poll after notifying the device, blocks on the
notification during DMA, clears the virtio-mmio interrupt status, ACKs the
handler, checks the used ring after the ACK, and only then reuses the shared
DMA/ring state. This deliberately preserves the current single in-flight request
invariant; supporting multiple outstanding requests should add explicit
per-request descriptors, completion matching, and shared-buffer ownership rules.

Verified commands:

```text
nix develop --command cargo fmt --check
nix develop --command cargo check -p kernel -p virtio-disk-server -p xv6-abi -p sel4-user
nix develop --command env TIMEOUT=180 LOG_FILE=target/xv6-m623-echo.log ./tools/run-xv6-user.sh echo irq disk
nix develop --command env TIMEOUT=1200 LOG_FILE=target/xv6-m623-usertests-full.log ./tools/run-xv6-user.sh usertests
nix develop --command cargo check -p virtio-disk-server -p xv6-abi -p sel4-user
nix develop --command env TIMEOUT=180 LOG_FILE=target/xv6-no-poll-echo.log ./tools/run-xv6-user.sh echo no-poll disk
nix develop --command env TIMEOUT=1200 LOG_FILE=target/xv6-no-poll-usertests.log ./tools/run-xv6-user.sh usertests
nix develop --command env TIMEOUT=180 LOG_FILE=target/xv6-m626-no-poll-ack-echo.log ./tools/run-xv6-user.sh echo no-poll ack
nix develop --command env TIMEOUT=1200 LOG_FILE=target/xv6-m626-no-poll-ack-usertests.log ./tools/run-xv6-user.sh usertests
```

The full run log `target/xv6-m623-usertests-full.log` ended with
`ALL TESTS PASSED` and `xv6-host: exit(0) pid=1`. The no-poll rerun log
`target/xv6-no-poll-usertests.log` ended the same way after removing the
remaining short spin window. The latest ACK-order rerun also ended cleanly
after changing completion wait to receive and ACK the virtio IRQ notification
before rechecking the used ring.

M6.24 tightens regular-file write semantics. `xv6-host` now handles
`FD_FS_SERVER_FILE` writes through xv6's `filewrite` chunk size
(`((MAXOPBLOCKS - 1 - 1 - 2) / 2) * BSIZE = 3072` bytes) instead of the old
128-byte generic syscall scratch buffer. `xv6-fs-server` accepts that
multi-block request through the existing shared page, copies it into a local
server buffer before disk reads overwrite the shared block window, and commits
the whole chunk in one redo-log transaction. If an inode write cannot complete
the requested chunk, the syscall returns `-1` as xv6 does, while preserving any
file offset advancement that occurred before the error.

This also clarified the current direct-`UPROGS` boundary for `logstress`:
with the unmodified upstream layout, `FSSIZE=2000`, `MAXFILE=268` blocks, and
the generated `fs.img` starts with 965 blocks already allocated. `logstress`
attempts 250 writes of 2000 bytes per file, or about 500KB per file, which
exceeds upstream `MAXFILE` even for `logstress f1 f2`, and the four-file
example also exceeds the remaining disk capacity. The observed `write failed
-1` is therefore now a capacity/on-disk-format limit, not the earlier host
128-byte chunking artifact.

Verified commands:

```text
nix develop --command cargo fmt --check
nix develop --command cargo check -p xv6-fs-server -p virtio-disk-server -p xv6-abi -p sel4-user
nix develop --command env TIMEOUT=900 LOG_FILE=target/xv6-m624-logstress.log ./tools/run-xv6-user.sh logstress f1 f2 f3 f4
nix develop --command env TIMEOUT=900 LOG_FILE=target/xv6-m624-logstress-2.log ./tools/run-xv6-user.sh logstress f1 f2
nix develop --command env TIMEOUT=1200 LOG_FILE=target/xv6-m624-usertests.log ./tools/run-xv6-user.sh usertests
```

The `logstress` runs fail with `write failed -1` after hitting upstream
`MAXFILE`/space limits. The full `usertests` rerun still ended with
`ALL TESTS PASSED` and `xv6-host: exit(0) pid=1`.

M6.25 adds xv6 orphan-inode recovery and cwd inode retention. `xv6-host`
retains each live process cwd inode through a new `FS_OP_RETAIN` operation,
duplicates that reference on `fork`, releases it on `chdir`, `exit`, and
process reap, and retains the initial root cwd before starting pid 1. This lets
`unlink` of a process's current directory or open file leave an on-disk inode
with `nlink == 0` while an in-memory reference is still live, as xv6 expects.
On the next `FS_OP_INIT`, after redo-log recovery, `xv6-fs-server` scans the
inode table and transactionally frees any remaining typed inode with
`nlink == 0`, logging `ireclaim: orphaned inode <inum>`.

Verified commands:

```text
nix develop --command cargo fmt
nix develop --command cargo check -p xv6-fs-server -p virtio-disk-server -p xv6-abi -p sel4-user
nix develop --command env TIMEOUT=8 LOG_FILE=target/xv6-m625-forphan-crash.log XV6_FS_IMG=target/xv6-m625-orphan.img ./tools/run-xv6-user.sh forphan
nix develop --command env TIMEOUT=180 LOG_FILE=target/xv6-m625-forphan-recover.log XV6_BUILD_FS_IMG=0 XV6_FS_IMG=target/xv6-m625-orphan.img ./tools/run-xv6-user.sh echo recover-forphan
nix develop --command env TIMEOUT=8 LOG_FILE=target/xv6-m625-dorphan-crash.log XV6_FS_IMG=target/xv6-m625-dorphan.img ./tools/run-xv6-user.sh dorphan
nix develop --command env TIMEOUT=180 LOG_FILE=target/xv6-m625-dorphan-recover.log XV6_BUILD_FS_IMG=0 XV6_FS_IMG=target/xv6-m625-dorphan.img ./tools/run-xv6-user.sh echo recover-dorphan
nix develop --command env TIMEOUT=1200 LOG_FILE=target/xv6-m625-usertests.log ./tools/run-xv6-user.sh usertests
```

The first `forphan`/`dorphan` phase intentionally times out after the program
prints its wait-for-crash line. The recovery phase reuses the same disk image
with `XV6_BUILD_FS_IMG=0` and logs `ireclaim: orphaned inode 22`. The full
`target/xv6-m625-usertests.log` ended with `ALL TESTS PASSED` and
`xv6-host: exit(0) pid=1`.

M6.26 continues the fs-server decomposition by moving IPC operation handlers
for open/close/read/write/readdir/fstat/chdir/exec lookup/link/unlink/mkdir/
mknod/retain into `userspace/xv6-fs-server/src/ops.rs`. `main.rs` is now back
to boot, `FS_OP_INIT`, orphan recovery, and dispatch glue, reducing it from
644 lines to 229 lines while preserving the existing fs-server semantics.

Verified commands:

```text
nix develop --command cargo fmt --check
nix develop --command cargo check -p xv6-fs-server -p virtio-disk-server -p xv6-abi -p sel4-user
nix develop --command env TIMEOUT=240 LOG_FILE=target/xv6-m626-fs-ops-split-stressfs.log ./tools/run-xv6-user.sh stressfs
nix develop --command env TIMEOUT=1200 LOG_FILE=target/xv6-m626-fs-ops-split-usertests.log ./tools/run-xv6-user.sh usertests
```

The `stressfs` run ended with `xv6-host: exit(0) pid=1`. The full
`target/xv6-m626-fs-ops-split-usertests.log` ended with `ALL TESTS PASSED`
and `xv6-host: exit(0) pid=1`.

M6.27 starts the host-side syscall decomposition by moving pipe ring state,
pipe allocation, reader/writer open-file backing counts, and raw pipe
read/write operations into `userspace/xv6-host/src/pipes.rs`. `xv6.rs` now
keeps only syscall-level blocking/resume and fd lifecycle decisions, preserving
the existing xv6 open-file semantics where `fork`/`dup` share one pipe backing
until the final close.

Verified commands:

```text
nix develop --command cargo fmt --check
nix develop --command cargo check -p xv6-fs-server -p virtio-disk-server -p xv6-abi -p sel4-user
nix develop --command env TIMEOUT=240 LOG_FILE=target/xv6-m627-pipes-split-pipe1.log ./tools/run-xv6-user.sh usertests pipe1
nix develop --command env TIMEOUT=1200 LOG_FILE=target/xv6-m627-pipes-split-usertests.log ./tools/run-xv6-user.sh usertests
```

The targeted `pipe1` run ended with `xv6-host: exit(0) pid=1`. The full
`target/xv6-m627-pipes-split-usertests.log` ended with `ALL TESTS PASSED`
and `xv6-host: exit(0) pid=1`.

M6.28 continues the host-side syscall decomposition by moving the open-file
backing table into `userspace/xv6-host/src/files.rs`. `xv6.rs` now keeps fd
policy and syscall blocking/resume behavior, while `files.rs` owns open-file
allocation, retain/release, forced close, and shared file offsets. This
preserves the xv6 semantics where `dup` and `fork` share one backing open file
until the final close, including shared offsets for `sharedfd`.

Verified commands:

```text
nix develop --command cargo fmt --check
nix develop --command cargo check -p xv6-fs-server -p virtio-disk-server -p xv6-abi -p sel4-user
nix develop --command env TIMEOUT=240 LOG_FILE=target/xv6-m628-files-split-sharedfd.log ./tools/run-xv6-user.sh usertests sharedfd
nix develop --command env TIMEOUT=1200 LOG_FILE=target/xv6-m628-files-split-usertests.log ./tools/run-xv6-user.sh usertests
```

The targeted `sharedfd` run ended with `xv6-host: exit(0) pid=1`. The full
`target/xv6-m628-files-split-usertests.log` ended with `ALL TESTS PASSED` and
`xv6-host: exit(0) pid=1`.

M6.29 tightens the IRQ-driven virtio completion path. The disk server still
submits only one virtqueue request at a time because the descriptor chain,
status byte, 1KiB DMA data window, and fs<->disk shared block page are singleton
resources. That concurrency boundary is now explicit: a scoped
`InFlightRequest` guard prevents accidental overlapping requests and releases
the flag on every return path. Completion wait first checks the used-ring index,
then blocks on the badged virtio IRQ notification only while the device still
owns the DMA request. On IRQ wakeup the server validates the badge, clears the
virtio-mmio interrupt status, ACKs the `IRQHandler`, and loops back to the used
ring check before reusing the DMA/ring state.

This keeps the driver non-polling during DMA while making the remaining
single-request assumption visible. Multiple in-flight disk requests should be a
separate change with a descriptor allocator, per-request status/data buffers,
completion-id matching, and explicit reply-cap ownership.

Verified commands:

```text
nix develop --command cargo fmt
nix develop --command cargo check -p xv6-fs-server -p virtio-disk-server -p xv6-abi -p sel4-user
nix develop --command env TIMEOUT=180 LOG_FILE=target/xv6-m629-irq-wait-echo.log ./tools/run-xv6-user.sh echo irq-wait guard
nix develop --command env TIMEOUT=1200 LOG_FILE=target/xv6-m629-irq-wait-usertests.log ./tools/run-xv6-user.sh usertests
```

The smoke run ended with `xv6-host: exit(0) pid=1`. The full
`target/xv6-m629-irq-wait-usertests.log` run also ended with
`ALL TESTS PASSED` and `xv6-host: exit(0) pid=1`.

M6.30 implements xv6-style `pause(n)` sleep semantics in `xv6-host`. A positive
pause duration now saves the syscall reply cap, records a tick deadline, marks
the process `PROC_SLEEPING`, and returns no reply until the deadline expires.
The host's bound timer notification already wakes the fault-endpoint receive
loop; on each timer badge the host increments the xv6 tick counter and resumes
all sleeping processes whose deadlines have passed. Cleanup paths also drop
saved sleep reply caps so `kill`, fault termination, and process reap do not
leave stale reply caps behind. The older compatibility behavior that advanced
the xv6 tick counter on every syscall has also been removed; `uptime()` and
`pause(n)` now advance from timer notifications.

This keeps `pause(0)` / negative pause as immediate success and makes programs
such as `zombie`, `grind`, `forphan`, and `dorphan` depend on the same timer
driven behavior as xv6 instead of an artificial single `Yield`.

Verified commands:

```text
nix develop --command cargo fmt
nix develop --command cargo check -p xv6-fs-server -p virtio-disk-server -p xv6-abi -p sel4-user
nix develop --command env TIMEOUT=180 LOG_FILE=target/xv6-m630-pause-zombie-timer-only.log ./tools/run-xv6-user.sh zombie
nix develop --command env TIMEOUT=240 LOG_FILE=target/xv6-m630-pause-preempt-timer-only.log ./tools/run-xv6-user.sh usertests preempt
nix develop --command env TIMEOUT=1200 LOG_FILE=target/xv6-m630-pause-usertests-timer-only.log ./tools/run-xv6-user.sh usertests
```

The `zombie` and targeted `preempt` runs ended with `xv6-host: exit(0) pid=1`.
The full `target/xv6-m630-pause-usertests-timer-only.log` run ended with
`ALL TESTS PASSED` and `xv6-host: exit(0) pid=1`.

M6.31 makes infinite xv6 stress programs usable as automated checks. `grind`
does not terminate by design, so `tools/run-xv6-user.sh` now accepts
`--expect-timeout` or `XV6_EXPECT_TIMEOUT=1`. In that mode a timeout is treated
as success only if the log does not contain fatal xv6/kernel output and the root
process did not exit. Normal mode keeps accepting full `usertests` logs where
intentional child fault kills are part of the test semantics; the stricter
`fault kill` / `grind:` fatal scan is only applied in expect-timeout mode.

Verified commands:

```text
bash -n tools/run-xv6-user.sh
nix develop --command cargo fmt --check
nix develop --command cargo check -p xv6-fs-server -p virtio-disk-server -p xv6-abi -p sel4-user
nix develop --command env TIMEOUT=90 LOG_FILE=target/xv6-m631-grind-expect-timeout.log ./tools/run-xv6-user.sh --expect-timeout grind
nix develop --command env TIMEOUT=1200 LOG_FILE=target/xv6-m631-grind-tooling-usertests-rerun.log ./tools/run-xv6-user.sh usertests
```

The `grind` run reported `PASS: timeout after 90s without fatal xv6 output`.
The full rerun log ended with `ALL TESTS PASSED` and
`xv6-host: exit(0) pid=1`.

M6.32 changes `virtio-disk-server` from synchronous disk RPC waiting to an
event-driven seL4 server loop. `xv6-host` now mints each service server a cap
to its own CNode at `XV6_SERVER_CNODE_CPTR`; the disk server uses that cap to
`CNode_SaveCaller` the fs-server reply cap into `XV6_SERVER_REPLY_CPTR`.
`xv6-host` also binds the badged virtio IRQ notification to the disk-server TCB.
For `DISK_OP_READ` / `DISK_OP_WRITE`, the disk server now validates the request,
saves the caller reply, submits the virtqueue descriptor chain, and returns to
`seL4_Recv(XV6_SERVICE_ENDPOINT_CPTR)` without blocking solely on the disk IRQ.
When the bound IRQ notification arrives, it clears the virtio-mmio interrupt
status, ACKs the IRQHandler, consumes the used-ring entry, copies read data back
to the shared page when needed, and replies through the saved reply cap.

The concurrency boundary remains explicit: there is still one shared descriptor
chain, one status byte, one 1KiB DMA data window, and one fs<->disk shared block
page, so a second block request while one is active is rejected instead of
overlapping DMA state. True multi-request virtqueue use remains future work and
needs descriptor/data/status allocation plus completion-id matching.

Verified commands:

```text
nix develop --command cargo fmt --check
nix develop --command cargo check -p xv6-fs-server -p virtio-disk-server -p xv6-abi -p sel4-user
nix develop --command env TIMEOUT=180 LOG_FILE=target/xv6-m632-virtio-async-echo.log ./tools/run-xv6-user.sh echo virtio async
nix develop --command env TIMEOUT=1200 LOG_FILE=target/xv6-m632-virtio-async-usertests.log ./tools/run-xv6-user.sh usertests
```

The smoke run ended with `xv6-host: exit(0) pid=1`. The full
`target/xv6-m632-virtio-async-usertests.log` run ended with
`ALL TESTS PASSED` and `xv6-host: exit(0) pid=1`; an additional log scan found
no kernel panic, kernel-mode trap, user fault, virtio completion failure,
spurious IRQ-completion diagnostic, or concurrent-request rejection.

Remaining work is further decomposition of host-side syscall dispatch and
server operation internals, plus the longer-running functional gaps around
interactive input, device coverage, resource scaling, optional larger files,
and multi-request virtio queueing.

M6.33 adds an explicit disk flush operation and fs redo-log write barriers.
`xv6-abi` now exposes `DISK_OP_FLUSH`, `VIRTIO_BLK_F_FLUSH`, and
`VIRTIO_BLK_T_FLUSH`. `virtio-disk-server` keeps the feature bit enabled during
negotiation when QEMU advertises it, records whether flush is supported, and
submits flush requests through the same event-driven reply-cap/IRQ-completion
path as read/write. If a device does not advertise flush, the server returns
success as a no-op so the fs layer can keep one simple durability protocol.

`xv6-fs-server` now uses flush barriers around the redo-log commit sequence:
after writing log data blocks, after writing the non-zero commit header, after
installing home blocks, and after clearing the log header. Recovery also flushes
after replaying home blocks and after clearing the header. This preserves the
single in-flight disk request invariant while making committed metadata updates
less dependent on QEMU/device writeback timing.

Verified commands:

```text
nix develop --command cargo fmt --check
nix develop --command cargo check -p xv6-fs-server -p virtio-disk-server -p xv6-abi -p sel4-user
nix develop --command env TIMEOUT=180 LOG_FILE=target/xv6-m633-disk-flush-echo.log ./tools/run-xv6-user.sh echo disk flush
nix develop --command env TIMEOUT=300 LOG_FILE=target/xv6-m633-disk-flush-stressfs.log ./tools/run-xv6-user.sh stressfs
nix develop --command env TIMEOUT=1200 LOG_FILE=target/xv6-m633-disk-flush-usertests.log ./tools/run-xv6-user.sh usertests
```

The `echo` and `stressfs` runs ended with `xv6-host: exit(0) pid=1`. The full
`target/xv6-m633-disk-flush-usertests.log` run ended with `ALL TESTS PASSED`
and `xv6-host: exit(0) pid=1`; a fatal-pattern scan found no disk flush
failure, virtio failure, unexpected IRQ completion, kernel panic, kernel-mode
trap, user fault, or concurrent-request rejection.

M6.34 prepares the virtio block path for real multi-request operation without
lying about the remaining shared-memory ownership constraints. `sel4-user` now
exports `msg_len`, and `xv6-abi` defines `XV6_DISK_MAX_IN_FLIGHT = 2` plus
`XV6_DISK_SHARED_BUFFER_SLOTS = 4`. The 4 KiB fs<->disk shared page is treated
as four 1 KiB block windows; disk read/write IPC accepts an optional fourth MR
selecting the shared slot, defaulting to slot 0 for backward compatibility.
`xv6-fs-server` now sends slot 0 explicitly.

Inside `virtio-disk-server`, the singleton pending request has been replaced by
a small request-slot table. Each request slot owns a descriptor-chain head,
request header, data DMA buffer, status byte, and saved reply-cap slot. Virtio
used-ring completion is matched by descriptor head id, and an IRQ can consume
and reply to multiple used entries before returning to the service endpoint.
The server rejects duplicate active shared-buffer slots so read completion
cannot overwrite an in-use caller-visible block window. Flush remains serialized
behind active data requests, preserving the barrier semantics added in M6.33.

Verified commands:

```text
nix develop --command cargo fmt
nix develop --command cargo check -p xv6-fs-server -p virtio-disk-server -p xv6-abi -p sel4-user
nix develop --command env TIMEOUT=180 LOG_FILE=target/xv6-m634-disk-slots-echo.log ./tools/run-xv6-user.sh echo disk slots
nix develop --command env TIMEOUT=1200 LOG_FILE=target/xv6-m634-disk-slots-usertests.log ./tools/run-xv6-user.sh usertests
```

The smoke run ended with `xv6-host: exit(0) pid=1`. The full
`target/xv6-m634-disk-slots-usertests.log` run ended with
`ALL TESTS PASSED` and `xv6-host: exit(0) pid=1`; a diagnostic scan found no
shared-slot conflict, request-slot exhaustion, unknown used-ring head,
unexpected IRQ completion, virtio failure, disk flush failure, kernel panic,
kernel-mode trap, user fault, or server panic.

M6.35 tightens the no-poll virtio completion/concurrency model. The disk server
still does not busy-wait after `QUEUE_NOTIFY`: once a request is submitted, it
returns to the seL4 receive loop and later completes via the bound virtio IRQ
notification. The new change is a single non-blocking used-ring harvest before
handling each non-IRQ disk RPC. This covers endpoint ordering where a DMA
completion is already visible but the IRQ badge is still queued behind another
client message, avoiding stale `request_slot` or `shared_slot` busy decisions
without spinning.

The request publication ordering is also stricter: read/write/flush now install
the `PENDING_REQUESTS` entry before the descriptor head is written to the avail
ring and before `avail.idx` is advanced. Completion lookup by descriptor head
therefore has valid software ownership as soon as the device can observe the
request. A later IRQ with no newly drained used entry is accepted as benign,
because the completion may have been harvested by the RPC path first.

Verified commands:

```text
nix develop --command cargo fmt
nix develop --command cargo check -p virtio-disk-server -p xv6-abi -p sel4-user
nix develop --command env TIMEOUT=180 LOG_FILE=target/xv6-m635-virtio-nopoll-echo.log ./tools/run-xv6-user.sh echo virtio nopoll
nix develop --command env TIMEOUT=300 LOG_FILE=target/xv6-m635-virtio-nopoll-stressfs.log ./tools/run-xv6-user.sh stressfs
```

Both targeted runs ended with `xv6-host: exit(0) pid=1`, and a diagnostic scan
of those logs found no virtio failure, disk flush failure, unknown used-ring
head, stale IRQ-completion diagnostic, request-slot exhaustion, shared-slot
conflict, kernel panic, kernel-mode trap, user fault, or seL4 call failure.
M6.36 below validates the later resource-scaling worktree with a full
`usertests` run.

M6.36 scales xv6-host resources toward upstream xv6's `param.h` while keeping
the service topology unchanged. `MAX_PROCS` is now 64, matching xv6 `NPROC`;
`MAX_EXEC_ARGS` is 32, matching xv6 `MAXARG`; and `MAX_PIPES` is 32. The
process table moved out of the host stack into static storage, per-process
control-object untyped memory was reduced to 256 KiB, and user data frames are
allocated from the largest non-device RAM untyped instead of each process
untyped.

The frame reuse path now records mapping ownership explicitly: only pages
allocated by the host's global process-frame allocator are returned to the
global frame pool. IPC-buffer frames, shared/device/server frames, and other
externally supplied caps are unmapped/deleted without entering that pool, so a
later process cannot reuse a cap invalidated by process-untyped revoke. Dead
`Mapping.proc_slot`, `Child.proc_slot`, and service proc-slot bookkeeping were
removed after this ownership split.

Verified commands:

```text
nix develop --command cargo fmt
nix develop --command cargo fmt --check
nix develop --command cargo check -p xv6-fs-server -p virtio-disk-server -p xv6-abi -p sel4-user
nix develop --command env TIMEOUT=180 LOG_FILE=target/xv6-m636-resource-scale-echo-clean.log ./tools/run-xv6-user.sh echo resource scale clean
nix develop --command env TIMEOUT=360 LOG_FILE=target/xv6-m636-twochildren.log ./tools/run-xv6-user.sh usertests twochildren
nix develop --command env TIMEOUT=1200 LOG_FILE=target/xv6-m636-resource-scale-usertests-clean.log ./tools/run-xv6-user.sh usertests
```

The final clean full run ended with `ALL TESTS PASSED` and
`xv6-host: exit(0) pid=1`. A diagnostic scan found no seL4 call failure,
mapping-table exhaustion, CSpace exhaustion, kernel panic, kernel-mode trap,
user fault, server panic, virtio failure, disk flush failure, unknown used-ring
head, stale IRQ-completion diagnostic, request-slot exhaustion, shared-slot
conflict, or xv6 failure marker.

M6.37 adds a small fs-server block cache on top of the fs->disk RPC boundary.
`read_disk_block()` now first checks the active transaction log, then a
16-entry clean block cache, and only then calls `virtio-disk-server`. Cache
hits copy the cached 1 KiB block back into the shared disk page so existing
inode, directory, and file helpers keep the same interface. Raw disk reads fill
the cache after a successful disk reply.

The cache is deliberately conservative around mutation. `log_write_shared()`
invalidates the affected home block instead of caching uncommitted data, so an
aborted transaction cannot leave dirty data visible. Successful raw writes,
including redo-log commit home-block installs and log-header updates, refresh
the cache with the committed shared-block image. This preserves the existing
xv6 redo-log ordering and still reduces repeated metadata reads.

Verified commands:

```text
nix develop --command cargo fmt
nix develop --command cargo fmt --check
nix develop --command cargo check -p xv6-fs-server -p virtio-disk-server -p xv6-abi -p sel4-user
nix develop --command env TIMEOUT=180 LOG_FILE=target/xv6-m637-block-cache-echo.log ./tools/run-xv6-user.sh echo block cache
nix develop --command env TIMEOUT=300 LOG_FILE=target/xv6-m637-block-cache-stressfs.log ./tools/run-xv6-user.sh stressfs
nix develop --command env TIMEOUT=1200 LOG_FILE=target/xv6-m637-block-cache-usertests.log ./tools/run-xv6-user.sh usertests
```

The `echo` and `stressfs` runs ended with `xv6-host: exit(0) pid=1`. The full
`target/xv6-m637-block-cache-usertests.log` run ended with
`ALL TESTS PASSED` and `xv6-host: exit(0) pid=1`; a diagnostic scan found no
seL4 call failure, mapping-table exhaustion, CSpace exhaustion, kernel panic,
kernel-mode trap, user fault, server panic, virtio failure, disk flush failure,
unknown used-ring head, stale IRQ-completion diagnostic, request-slot
exhaustion, shared-slot conflict, or xv6 failure marker.

M6.38 makes xv6 smoke/full-suite tooling safe for parallel local runs. The
helper scripts now share `tools/xv6-build-lock.sh`, a small reentrant lock around
the mutable xv6 build tree, Cargo rootserver payload rebuilds, upstream
`fs.img` generation, and the shared sel4test elfloader staging directory used
by `tools/pack-image.sh`. `run-xv6-user.sh` holds that lock only through
rootserver build, fs image build/copy, and image packing; QEMU runs after the
lock is released.

Default `run-xv6-user.sh` invocations also use a per-run kernel image name and a
per-run `fs.img` copy for QEMU. That avoids concurrent QEMU instances writing
the same raw xv6 disk image. Explicit `XV6_FS_IMG=...` keeps the old behavior,
which is still required for persistence/recovery tests such as the
`forphan`/`dorphan` crash+recover flow. `XV6_KEEP_RUN_FS_IMG=1` keeps the
temporary per-run image for inspection.

Verified commands:

```text
bash -n tools/xv6-build-lock.sh
bash -n tools/build-xv6-fs-img.sh
bash -n tools/build-xv6-user-rootserver.sh
bash -n tools/run-xv6-user.sh
nix develop --command env TIMEOUT=180 LOG_FILE=target/xv6-m638-tool-lock-echo.log ./tools/run-xv6-user.sh echo tool lock
nix develop --command bash -c 'set -euo pipefail; env TIMEOUT=180 LOG_FILE=target/xv6-m638-parallel-echo.log ./tools/run-xv6-user.sh echo parallel lock & p1=$!; env TIMEOUT=300 LOG_FILE=target/xv6-m638-parallel-stressfs.log ./tools/run-xv6-user.sh stressfs & p2=$!; wait "$p1"; wait "$p2"'
nix develop --command cargo fmt --check
```

The sequential `echo` run and the parallel `echo`/`stressfs` run all ended with
`xv6-host: exit(0) pid=1`. After the parallel run, the build lock directory was
gone and the default per-run fs images had been cleaned up.

M6.39 wires dynamic host console input through the microkernel path. The kernel
now exposes a local debug `SYS_DEBUG_GET_CHAR` syscall backed by OpenSBI
`console_getchar`, `sel4-user` wraps it as `getchar()`, and `xv6-host` uses it
only in user space to implement xv6 console `read()`. If no build-time
`XV6_CONSOLE_INPUT` is embedded, a console read saves the fault reply cap,
records the destination buffer, marks the process `PROC_CONSOLE_READ`, and
returns to the host receive loop. Timer notifications then poll debug getchar
without blocking; when a byte is available, the host copies it into the child,
echoes it, and replies through the saved cap.

`tools/run-xv6-user.sh` also has `--qemu-stdin` and `--qemu-stdin-file` for
tests that must feed runtime QEMU stdin rather than compile-time scripted
console input. Since EOF on a UART-like console is treated as no input, these
tests normally use `--expect-timeout` and then inspect the log for the command
that should have run.

Verified commands:

```text
bash -n tools/run-xv6-user.sh
nix develop --command cargo fmt --check
nix develop --command cargo check -p kernel -p sel4-user -p xv6-fs-server -p virtio-disk-server -p xv6-abi
nix develop --command env TIMEOUT=180 LOG_FILE=target/xv6-m639-console-scripted.log ./tools/run-xv6-user.sh --stdin 'echo scripted console\n' sh
nix develop --command env TIMEOUT=60 LOG_FILE=target/xv6-m639-console-dynamic-qemu-stdin2.log ./tools/run-xv6-user.sh --expect-timeout --qemu-stdin $'\necho dynamic console\n' sh
rg -n 'dynamic console|xv6-host: exec echo|xv6-host: exit|fault kill|KERNEL PANIC|user fault' target/xv6-m639-console-dynamic-qemu-stdin2.log
```

The scripted shell run exited with `xv6-host: exit(0) pid=1`. The dynamic QEMU
stdin run timed out as expected after the shell returned to waiting for further
console input, and its log showed `$ echo dynamic console`,
`xv6-host: exec echo pid=2`, `dynamic console`, and `xv6-host: exit(0) pid=2`
with no fatal diagnostics.

M6.40 tightens xv6-host process control-object cleanup. The child IPC-buffer
frame is now stored in `Child` and mapped directly into the child VSpace rather
than being registered as a normal user mapping. This keeps `MAPPINGS` focused on
payload/heap/stack pages, prevents `exec()`/reap mapping cleanup from owning a
control-object cap, and lets `destroy_child_objects()` explicitly delete the IPC
frame slot exactly once. Fork failure cleanup also clears any child mappings
created before destroying the partially constructed TCB/CNode/VSpace objects.

This fixes the recycled-slot aliasing exposed by repeated shell `forktest`
runs: the old attempted ownership split could delete the IPC frame once through
`clear_process_mappings()` and again through `destroy_child_objects()`, causing
a later CNode copy to see an invalid source cap. The new ownership boundary is
one object owner per cap slot: process mappings own user frame caps, and
`Child` owns TCB/CNode/VSpace/IPC-frame control caps.

Verified commands:

```text
nix develop --command cargo fmt
nix develop --command cargo fmt --check
nix develop --command cargo check -p kernel -p sel4-user -p xv6-fs-server -p virtio-disk-server -p xv6-abi
nix develop --command env TIMEOUT=240 LOG_FILE=target/xv6-m640-ipc-frame-forktest.log ./tools/run-xv6-user.sh forktest
nix develop --command env TIMEOUT=360 LOG_FILE=target/xv6-m640-ipc-frame-shell-forktest-repeat3.log ./tools/run-xv6-user.sh --stdin $'forktest\nforktest\nforktest\nforktest\nforktest\n' sh
rg -c 'fork test OK' target/xv6-m640-ipc-frame-shell-forktest-repeat3.log
```

The direct `forktest` exited with `xv6-host: exit(0) pid=1`. The repeated shell
run reported five `fork test OK` lines and ended with
`xv6-host: exit(0) pid=1`, with no `seL4 call failed`, CSpace exhaustion,
mapping-table exhaustion, kernel panic, kernel-mode trap, or user fault.

M6.41 clarifies the virtio no-poll invariant. The disk server still returns to
`seL4_Recv` while DMA is in flight and resumes completion from the bound virtio
IRQ notification. The pre-RPC used-ring harvest is deliberately non-blocking:
it only handles the endpoint ordering case where a device completion is already
visible but the IRQ badge is queued behind a client message, so request-slot
and shared-slot ownership stay current without spinning.

M6.42 splits fs-server shared-buffer ownership. Shared slots 0..2 are now
reserved for host/fs syscall payloads and replies, covering xv6's 3072-byte
`filewrite` chunks, while fs/disk block I/O uses shared slot 3. This lets
`FS_OP_WRITE` consume the host-provided write payload directly while inode,
bitmap, directory, log, and cache disk operations use a separate block window,
removing the former `WRITE_BUFFER` copy. `FS_OP_READ` now explicitly copies the
requested bytes from the disk block window back to the host-visible slot 0
before replying. The fs server is still synchronous around disk RPCs; this
change removes the shared-memory aliasing constraint needed before a real async
fs-server state machine can be introduced.

Verified commands:

```text
nix develop --command cargo fmt
nix develop --command cargo check -p xv6-fs-server -p virtio-disk-server -p xv6-abi -p sel4-user
nix develop --command env TIMEOUT=180 LOG_FILE=target/xv6-m642-shared-slots-cat.log ./tools/run-xv6-user.sh cat README
nix develop --command env TIMEOUT=300 LOG_FILE=target/xv6-m642-shared-slots-stressfs.log ./tools/run-xv6-user.sh stressfs
nix develop --command env TIMEOUT=240 LOG_FILE=target/xv6-m642-shared-slots-bigwrite.log ./tools/run-xv6-user.sh usertests bigwrite
nix develop --command env TIMEOUT=1200 LOG_FILE=target/xv6-m642-shared-slots-usertests-rerun.log ./tools/run-xv6-user.sh usertests
rg -n "virtio failure|disk flush failed|used entry for unknown|request slots exhausted|shared slot busy|KERNEL PANIC|kernel-mode trap|user fault|seL4 call failed|xv6-host: panic|xv6-fs-server: panic|virtio-disk-server: panic" target/xv6-m642-shared-slots-bigwrite.log target/xv6-m642-shared-slots-usertests-rerun.log
```

`cat README`, `stressfs`, and targeted `usertests bigwrite` ended with
`xv6-host: exit(0) pid=1`, covering the readback path from disk slot 3 to host
slot 0 and the 3072-byte write path that keeps host payload bytes stable while
disk metadata/data blocks move through slot 3. The full
`target/xv6-m642-shared-slots-usertests-rerun.log` run ended with
`ALL TESTS PASSED` and `xv6-host: exit(0) pid=1`; the fatal-pattern scan above
returned no matches. An earlier full run caught the initial slot-1 overlap as
`xv6-fs-server: panic` in `bigwrite`, which is why the final disk work slot is
slot 3 rather than slot 1.

M6.43 adds an explicit disk-completion IPC path while keeping virtio DMA
event-driven. `xv6-host` now mints `virtio-disk-server` a badged send cap to
the fs-server endpoint. `xv6-fs-server` sends disk read/write/flush requests
with a completion id instead of using `seL4_Call`, then waits for
`DISK_OP_COMPLETE` on its own service endpoint. `virtio-disk-server` keeps the
old synchronous reply-cap path for compatibility, but async requests complete by
sending `[status, bytes, blockno, completion_id, detail]` back to that endpoint.

The disk server still does not poll while DMA is in flight. After publishing a
virtqueue descriptor, it returns to `seL4_Recv(XV6_SERVICE_ENDPOINT_CPTR)` and
can receive either a virtio IRQ notification or another client IPC. Completion
processing is driven by the bound IRQ badge; the pre-RPC used-ring drain remains
a non-blocking ordering fix for the case where a device completion is already
visible but the IRQ badge is queued behind an IPC message. Concurrency ownership
is still explicit: `PENDING_REQUESTS` owns each request slot until its used-ring
head is consumed, shared slots cannot be reused while active, and flush remains
a barrier that is rejected while data requests are in flight.

This step does not yet make `xv6-fs-server` a fully asynchronous state machine.
The fs block layer now avoids `seL4_Call` to the disk server, but the fs server
still waits synchronously for the matching completion before replying to
`xv6-host`. The next concurrency step is to save the host caller reply cap,
record pending fs operation state, and let the fs-server main loop process other
host IPC while disk requests are outstanding.

Verified commands:

```text
nix develop --command cargo fmt --check
nix develop --command cargo check -p xv6-fs-server -p virtio-disk-server -p xv6-abi -p sel4-user
nix develop --command env TIMEOUT=180 LOG_FILE=target/xv6-m643-disk-completion-cat.log ./tools/run-xv6-user.sh cat README
nix develop --command env TIMEOUT=300 LOG_FILE=target/xv6-m643-disk-completion-stressfs.log ./tools/run-xv6-user.sh stressfs
nix develop --command env TIMEOUT=1200 LOG_FILE=target/xv6-m643-disk-completion-usertests.log ./tools/run-xv6-user.sh usertests
rg -n "virtio failure|disk flush failed|used entry for unknown|request slots exhausted|shared slot busy|unexpected disk completion|KERNEL PANIC|kernel-mode trap|user fault|seL4 call failed|xv6-host: panic|xv6-fs-server: panic|virtio-disk-server: panic" target/xv6-m643-disk-completion-cat.log target/xv6-m643-disk-completion-stressfs.log target/xv6-m643-disk-completion-usertests.log
```

`cat README`, `stressfs`, and the full `usertests` run all ended with
`xv6-host: exit(0) pid=1`; the full `usertests` log also ended with
`ALL TESTS PASSED`. The fatal-pattern scan above returned no matches.

M6.44 hardens the fs-server disk-wait path for future host/fs concurrency.
`sel4-user` now exposes a minimal `seL4_Reply` wrapper, and `xv6-abi` defines
`XV6_EBUSY`. While `xv6-fs-server` is waiting for a specific
`DISK_OP_COMPLETE`, it now loops on its service endpoint instead of treating the
first non-matching message as the current disk operation's failure. A wrong disk
completion id is still a protocol error, but an unrelated host/fs IPC is replied
to immediately with `XV6_EBUSY` and the original disk wait continues.

This is intentionally a guard, not the final async fs-server scheduler. It
prevents shared slot 3, the transaction log, and the block cache from being
re-entered by a second fs operation while one disk request is outstanding. The
next step is to convert fs operations into saved-reply continuations instead of
replying busy to concurrent fs work.

Verified commands:

```text
nix develop --command cargo fmt --check
nix develop --command cargo check -p xv6-fs-server -p virtio-disk-server -p xv6-abi -p sel4-user
nix develop --command env TIMEOUT=180 LOG_FILE=target/xv6-m644-fs-busy-guard-cat.log ./tools/run-xv6-user.sh cat README
nix develop --command env TIMEOUT=1200 LOG_FILE=target/xv6-m644-fs-busy-guard-usertests.log ./tools/run-xv6-user.sh usertests
rg -n "busy while disk pending|virtio failure|disk flush failed|used entry for unknown|request slots exhausted|shared slot busy|unexpected disk completion|KERNEL PANIC|kernel-mode trap|user fault|seL4 call failed|xv6-host: panic|xv6-fs-server: panic|virtio-disk-server: panic" target/xv6-m644-fs-busy-guard-cat.log target/xv6-m644-fs-busy-guard-usertests.log
```

`cat README` ended with `xv6-host: exit(0) pid=1`. The full
`target/xv6-m644-fs-busy-guard-usertests.log` run ended with
`ALL TESTS PASSED` and `xv6-host: exit(0) pid=1`; the fatal-pattern scan above
returned no matches.

M6.45 makes the host side understand the fs-server busy guard. All host->fs
IPC calls now go through a centralized `fs_server_call()` helper; replies with
`XV6_EBUSY` cause `xv6-host` to `seL4_Yield` and retry up to a bounded limit,
while non-busy fs statuses keep their previous syscall semantics. This keeps
the M6.44 guard from leaking as ordinary xv6 I/O failures and prepares the path
for fs-server saved-reply continuations.

M6.46 removes the remaining blocking edge from disk completion delivery.
`xv6-host` now allocates and maps a dedicated completion ring page at
`XV6_DISK_COMPLETION_RING_VADDR`, gives `virtio-disk-server` a badged send cap
to a completion Notification, and binds that Notification to the fs-server TCB.
On used-ring completion, `virtio-disk-server` writes
`[status, bytes, blockno, completion_id, detail]` into the ring and signals the
Notification instead of blocking in an endpoint `Send`. `xv6-fs-server` drains
the ring while waiting for its outstanding disk completion and ignores stale
completion notification badges in its main loop, so a notification delivered
after an opportunistic ring drain is not mistaken for `FS_OP_INIT`.

The fs server is still intentionally single-outstanding around disk I/O: shared
slot 3, the transaction log, and the block cache are protected by `XV6_EBUSY`
rather than a full continuation scheduler. The next step is to save host reply
caps and convert fs operations into explicit continuations so unrelated host/fs
IPC can make progress while a disk request is in flight.

Verified commands:

```text
nix develop --command cargo fmt --check
nix develop --command cargo check -p xv6-fs-server -p virtio-disk-server -p xv6-abi -p sel4-user
nix develop --command env TIMEOUT=180 LOG_FILE=target/xv6-m646-disk-completion-ring-cat.log ./tools/run-xv6-user.sh cat README
nix develop --command env TIMEOUT=300 LOG_FILE=target/xv6-m646-disk-completion-ring-stressfs.log ./tools/run-xv6-user.sh stressfs
nix develop --command env TIMEOUT=1200 LOG_FILE=target/xv6-m646-disk-completion-ring-usertests.log ./tools/run-xv6-user.sh usertests
rg -n "completion ring full|unexpected disk completion|unexpected disk completion ring|fs busy retry exhausted|busy while disk pending|virtio failure|disk flush failed|used entry for unknown|request slots exhausted|shared slot busy|KERNEL PANIC|kernel-mode trap|user fault|seL4 call failed|xv6-host: panic|xv6-fs-server: panic|virtio-disk-server: panic" target/xv6-m646-disk-completion-ring-cat.log target/xv6-m646-disk-completion-ring-stressfs.log target/xv6-m646-disk-completion-ring-usertests.log
```

`cat README`, `stressfs`, and full `usertests` all ended with
`xv6-host: exit(0) pid=1`; full `usertests` also ended with
`ALL TESTS PASSED`. The fatal-pattern scan above returned no matches.

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
| M6.20 | First xv6-fs-server module split: disk/runtime FS types and statics moved to `types.rs`; transaction, redo-log, disk IPC, and shared-block helpers moved to `block.rs`. Full `usertests` still passes. | ✅ Done |
| M6.21 | Inode-layer split: raw dinode IO, open-reference tracking, inode allocation/free, truncation, data writes, bitmap allocation, and direct/indirect block mapping moved to `inode.rs`. Full `usertests` still passes. | ✅ Done |
| M6.22 | Directory/path split: dirent read/write, root/path/parent lookup, empty-directory checks, directory entry insertion/removal, node creation, and path-byte logging moved to `dir.rs`. Full `usertests` still passes. | ✅ Done |
| M6.23 | IRQ-driven virtio disk completion: add minimal PLIC external IRQ delivery, complete PLIC IRQs through `IRQHandler_Ack`, pass a badged virtio IRQ notification/handler cap to `virtio-disk-server`, and block the disk server on notification instead of polling DMA completion. Full `usertests` still passes. | ✅ Done |
| M6.24 | xv6 `filewrite` chunk semantics: regular-file writes use the upstream 3072-byte max transaction chunk, fs-server accepts multi-block write IPC via the shared page plus a local buffer, and partial inode writes return `-1` instead of leaking internal short counts. Full `usertests` still passes; `logstress` is now blocked by upstream `MAXFILE`/FSSIZE limits rather than host chunking. | ✅ Done |
| M6.25 | xv6 orphan recovery: `xv6-host` retains cwd inode refs for live processes and releases them on cwd/process lifetime transitions; `xv6-fs-server` reclaims post-crash `nlink == 0` typed inodes after log recovery. `forphan`, `dorphan`, and full `usertests` pass. | ✅ Done |
| M6.26 | Fs-server operation split: move open/close/read/write/readdir/fstat/chdir/exec lookup/link/unlink/mkdir/mknod/retain IPC handlers into `ops.rs`, leaving `main.rs` to boot/init/orphan recovery/dispatch. `stressfs` and full `usertests` pass. | ✅ Done |
| M6.27 | Host pipe split: move pipe ring state, allocation, reader/writer backing counts, and raw pipe read/write operations into `xv6-host/src/pipes.rs`, keeping syscall block/resume policy in `xv6.rs`. Targeted `pipe1` and full `usertests` pass. | ✅ Done |
| M6.28 | Host open-file split: move open-file backing allocation, refcounts, forced close, and shared offsets into `xv6-host/src/files.rs`, keeping fd/syscall policy in `xv6.rs`. Targeted `sharedfd` and full `usertests` pass. | ✅ Done |
| M6.29 | Virtio completion wait cleanup: keep one request in flight with a scoped guard, check the used ring before blocking, then sleep on the badged IRQ notification during DMA and ACK the handler before reusing shared DMA state. | ✅ Done |
| M6.30 | xv6 `pause(n)` semantics: positive pause saves the reply cap, blocks the process until timer notifications advance ticks past its deadline, removes syscall-driven pseudo-ticks, and cleans up saved sleep replies on kill/reap. `zombie`, targeted `preempt`, and full `usertests` pass. | ✅ Done |
| M6.31 | `run-xv6-user.sh --expect-timeout` validation mode for infinite stress programs such as `grind`; timeout is success only when no fatal xv6/kernel output is seen, while normal full `usertests` still tolerates intentional child fault-kill logs. | ✅ Done |
| M6.32 | Event-driven virtio disk server: save the fs-server reply cap, submit DMA, return to the service endpoint, receive the bound IRQ notification through the same server loop, then ACK and reply through the saved cap. One request remains in flight. Full `usertests` passes. | ✅ Done |
| M6.33 | Disk flush + fs log barriers: add `DISK_OP_FLUSH`, negotiate `VIRTIO_BLK_F_FLUSH` with no-op fallback, and flush after log data, commit header, home install, and log clear. `stressfs` and full `usertests` pass. | ✅ Done |
| M6.34 | Disk request-slot groundwork: split the shared block page into explicit 1KiB shared slots, track two virtio request slots with per-slot DMA/status/reply caps, match used-ring completions by descriptor head, and keep flush serialized as a barrier. Full `usertests` passes. | ✅ Done |
| M6.35 | No-poll virtio concurrency tightening: opportunistically harvest already-visible used-ring completions before new disk RPC handling, publish pending request state before `avail.idx`, and tolerate IRQ badges whose completion was drained by the RPC path. Targeted `echo` and `stressfs` pass. | ✅ Done |
| M6.36 | xv6-host resource scaling: align `MAX_PROCS=64`, `MAX_EXEC_ARGS=32`, and `MAX_PIPES=32`, move the process table to static storage, allocate user frames from global RAM, and recycle only pool-owned mappings. Full `usertests` passes. | ✅ Done |
| M6.37 | Fs-server clean block cache: cache successful disk reads, prefer active transaction-log blocks, invalidate home-block cache entries on journal absorption, and refresh cache after committed raw writes. `echo`, `stressfs`, and full `usertests` pass. | ✅ Done |
| M6.38 | Parallel-safe xv6 run tooling: add a reentrant build/pack lock, use per-run packed image names, and give default QEMU runs private fs.img copies while preserving explicit `XV6_FS_IMG` persistence semantics. Sequential `echo` and parallel `echo`/`stressfs` pass. | ✅ Done |
| M6.39 | Dynamic console input: add local debug getchar plumbing, block xv6 console reads on saved reply caps, resume them from timer-driven host polling, and add `run-xv6-user.sh --qemu-stdin` for runtime stdin validation. Scripted shell and dynamic QEMU-stdin shell smoke pass. | ✅ Done |
| M6.40 | xv6-host control-cap ownership: keep child IPC-buffer frames out of the normal user mapping table, store/delete them as `Child` control caps, and clear partial child mappings on fork-construction failure. Direct `forktest` and five repeated shell `forktest` runs pass. | ✅ Done |
| M6.41 | Virtio no-poll invariant cleanup: document the disk server's event-driven DMA path in code and README. The pre-RPC used-ring harvest is explicitly non-blocking and only resolves endpoint/IRQ ordering races; DMA completion still resumes via the bound virtio IRQ notification and saved reply caps. | ✅ Done |
| M6.42 | Fs-server shared-slot split: reserve slots 0..2 for host/fs payloads, move fs/disk block I/O to slot 3, remove the write-side `WRITE_BUFFER` copy, and copy read replies explicitly from the disk slot back to the host slot. `cat README` and `stressfs` pass. | ✅ Done |
| M6.43 | Explicit disk completion channel: fs-server disk I/O now uses `Send + DISK_OP_COMPLETE` with completion ids instead of `seL4_Call`, while virtio DMA remains IRQ-driven and request/shared-slot ownership prevents in-flight reuse. Full `usertests` passes. | ✅ Done |
| M6.44 | Fs-server disk-wait concurrency guard: add user-space `seL4_Reply`, return `XV6_EBUSY` to unrelated host/fs IPC while a disk completion is pending, and keep the original disk wait alive. Full `usertests` passes. | ✅ Done |
| M6.45 | Host fs-busy retry: centralize host->fs IPC, retry `XV6_EBUSY` with `seL4_Yield`, and preserve normal fs failure semantics for non-busy replies. `cat README` and full `usertests` pass. | ✅ Done |
| M6.46 | Nonblocking disk completion delivery: replace disk->fs blocking completion endpoint sends with a shared completion ring plus bound Notification, while keeping fs-server single-outstanding disk concurrency guarded by `XV6_EBUSY`. `cat README`, `stressfs`, and full `usertests` pass. | ✅ Done |
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
zombie/blocking `wait`, scripted and runtime QEMU console input, console/file
read-write where meaningful, per-process `open`/`close`/`dup`/`fstat`, shared
open-file offsets across `dup`/`fork`, `sbrk`, `getpid`, `uptime`, `pause`,
`chdir`,
`mknod("console")`, `link`/`unlink`/`mkdir`, mutable files/directories through
the fs server, and a fixed-size in-memory `pipe` ring buffer with blocking
reads/writes shared across forked processes. Process termination now uses one
path for normal `exit`, fault kill, and `kill(pid)`, including wait reply,
child reparenting, and blocked reply-cap cleanup.
Remaining Unix gaps are permissions/devices beyond console, full
untyped-memory reclamation beyond cap-slot reuse, multi-request virtio queueing
beyond the current fs-server synchronous disk RPC loop, optional
large-file/on-disk-format expansion beyond upstream `MAXFILE`, and further
dynamic scaling beyond the current xv6-sized fixed tables. With no scripted
input, `init` now reaches `exec("sh")` and the shell blocks on runtime console
read until QEMU/stdin provides bytes.


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
│       │   ├── plic.rs        # QEMU virt PLIC MMIO helper for external IRQs
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
6. **xv6 service split.** The in-host filesystem mirror is gone, fs mutations
   use the fs server's xv6 redo-log path, and virtio disk completion is now
   IRQ-driven without spin-polling after submit. Fs/disk I/O now uses explicit
   completion ids, and disk completion delivery uses a shared completion ring
   plus a bound Notification rather than a blocking endpoint send. The fs
   server still waits synchronously for each disk completion before replying to
   the host; unrelated fs IPC during that wait is answered with `XV6_EBUSY`,
   and `xv6-host` now yields/retries that busy status. Console input can now
   arrive through runtime QEMU stdin via the host's debug-getchar path. Next xv6
   work is splitting the remaining large fs-server/host handlers further, then
   converting disk waits into saved-reply continuations so unrelated fs work can
   proceed while disk DMA is in flight.
