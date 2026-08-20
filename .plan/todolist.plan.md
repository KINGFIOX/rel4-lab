主路径已经和上游 **non-MCS seL4** 对齐；真正还没接上、又会让 seL4 用户态踩坑的，集中在 **SMP invocation 编号**、**BootInfo extra BI** 和 **IRQ 面偏窄**。策略上的差距（无 MCS、无优先级、单 domain）是故意的，不要当成缺功能去补。

完整对照表在 [rel4 与上游 seL4 差距](/Users/wangfiox/.cursor/projects/Users-wangfiox-microkernel/canvases/sel4-upstream-gap.canvas.tsx)，可在聊天旁打开筛选「对齐 / 策略 / 未完成」。条目多，用画布比在对话里堆表格更清楚。

**已经对齐的**

CSpace / MDB / zombie / untyped（含 100 工作单元抢占点）、non-MCS Endpoint / Notification / Reply、故障 label 1/2/3/5、RV64 Sv39 与 x86 四级用户映射、BKL + remote stall + FPU/TLB IPI、lazy FPU，以及 **timeslice**：`timer_tick` 对上游 `timerTick`（默认 5 拍）。仍可运行的线程会占着 CPU，直到时间片耗尽、Yield、阻塞或 suspend。旧笔记里「每个 trap 都 RR」已经不是现状。

**故意不同（策略）**

- 无 MCS / SchedContext / timeout fault（`pack-image.py` 钉死 `MCS=OFF`）
- `TCBSetPriority` / MCP / SchedParams、`DomainSet` 会成功，但不参与调度
- 无优先级抢占：新唤醒线程只入队尾

**还没对齐、又影响「想跑 seL4 用户态」的**

1. **`TCBSetAffinity`：内核里有 `set_affinity`，invocation 没接。** 上游在 non-MCS + SMP 下把 Affinity 插在 15，后面全体 +1。rel4 和 `userspace/sel4-user` 锁在 SMP-off（15=TLS，17=CNodeRevoke）。x86 打包默认 `SMP=OFF`，对得上。RISC-V 默认 `SMP=ON`：注入的是 Rust 内核，sel4test 用户态按 CMake 生成的 libsel4 编——这是目前最实在的 ABI 缝。
2. **BootInfo extra BI 不完整。** elfloader 把 DTB 传进 `init_kernel`，RISC-V 启动只记日志，没有上游那种 `SEL4_BOOTINFO_HEADER_FDT`。x86 写了 TSC 频率页，但 `extra_bi_pages` 仍是 `{0,0}`。
3. **IRQ 比上游窄。** RISC-V 泛型 Get 可用；GetTrigger 直接 `IllegalOperation`。上游 qemu-virt 用的 `plic0` 带 `HAVE_SET_TRIGGER`，会编程 trigger 并发 handler。x86 泛型 Get 只发 LAPIC timer，GetIOAPIC 有了，GetMSI 只有 label。
4. **调试 syscall 是桩。** DumpScheduler / Snapshot 空成功；SendIpi 两边都 halt。没有 fastpath。Idle TCB 在，空转留在内核 WFI/HLT，不经 `sret`/`sysret` 回 idle 用户上下文。

**验证面（2026-08-20 重跑后）**

`timer_tick` 对齐上游 `timerTick`。仍可运行的线程会占着 CPU，直到时间片耗尽、Yield、阻塞或 suspend。普通 trap 不再轮转。

`REL4_HAS_TIMER_PREEMPTION` 已改为 1。RV64 unicore（`SMP=OFF`）上：

- `FPU0001` 通过（时间片会切开并行 FPU 线程）。
- `SCHED0021` 通过（临时拿掉上游 `!CONFIG_SIMULATION` 才编进去）。默认 qemu-virt 仍 `SIMULATION=ON`，所以上游那道闸会把它排除；不是 rel4 时间片不够。
- `PREEMPT_REVOKE` 在 RISC-V virt 上编不进去：`LibPlatSupportHaveTimer=OFF`。x86（有用户态 timer）上跑了：revoke 线程一直占 CPU，等待线程靠优先级 101>100 抢不进来，cnode 越做越大直到 OOM（`seL4 error 2` / TCB configure 失败）。已改挂 `REL4_HAS_PRIORITY_SCHEDULING`，不再用时间片开关误关。

`REL4_HAS_PRIORITY_SCHEDULING=0` 关 `SCHED0003`–`0006` / `0020` 仍是对的。x86 默认 regex 仍只跑一小段 IPC/CNode/`TIMER0001`；同机 `FPU0001` 在 600s 内没等到足够中途抢占（迭代加倍一直建线程），先记成 x86 验证缺口，不据此关 RISC-V。

**多出来的（不是缺口）**

linux-compat 把未知 Linux syscall 变成 `UnknownSyscall` 故障 IPC，交给用户态服务器。这是 rel4 多的面。

若目标是更接近上游用户态，建议顺序是：先把 SMP 编号和 `TCBSetAffinity` 说清楚，再补 RISC-V FDT extra BI，然后让 GetTrigger 至少能发出 handler。VTX / IOMMU / MCS 只有在明确要虚拟化或实时时才值得碰。