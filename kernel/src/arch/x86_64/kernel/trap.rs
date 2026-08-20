//! x86_64 trap handling: IDT, `syscall`/`sysret`, and the seL4 user context.

use core::arch::global_asm;
use core::mem::size_of;
use core::sync::atomic::{AtomicBool, Ordering};

use log_crate::{error, warn};

use crate::abi::constants::{N_TOTAL_MSG_REGISTERS, WORD_BYTES};
use crate::abi::fault::FaultLabel;
use crate::abi::syscall::SyscallNumber;
use crate::abi::types::MessageInfo;
use crate::api::cspace;
use crate::arch::x86_64::machine::{irq, lapic, registers};
use crate::arch::x86_64::object::vspace;
use crate::arch::x86_64::sel4_arch::{UserContext, UserRegister};
use crate::ktypes::once::BootOnce;
use crate::ktypes::percpu::{PerCpu, PerCpuInit};
use crate::object::cap::{Cap, CapTag};
use crate::object::endpoint::{EndpointRef, EpState};
use crate::object::tcb::TcbRef;

#[allow(unused_imports)]
pub use crate::arch::x86_64::sel4_arch::same_object_as;

const SYSCALL_CAUSE: u64 = 0x10000;
const VEC_DEVICE_NOT_AVAILABLE: u64 = 7;
const VEC_DOUBLE_FAULT: u64 = 8;
const VEC_GENERAL_PROTECTION: u64 = 13;
const VEC_PAGE_FAULT: u64 = 14;
const FAULT_MR_REG_COUNT: u64 = 4;

const KERNEL_CS: u16 = 0x08;
const KERNEL_DS: u16 = 0x10;
const USER_DS: u16 = 0x18;
const USER_CS: u16 = 0x20;
const TSS_SEL: u16 = 0x28;
const STAR_USER_BASE: u64 = 0x10;
const FMASK: u64 = 0x4_7700;

const _: () = {
    assert!(core::mem::offset_of!(UserContext, regs) == 0);
    assert!(core::mem::offset_of!(UserContext, pc) == 24 * 8);
    assert!(core::mem::offset_of!(UserContext, restart_pc) == 25 * 8);
};

global_asm!(include_str!("../trap.S"), options(att_syntax));

unsafe extern "C" {
    pub fn restore_user_context(ctx: *mut UserContext) -> !;
    fn restore_user_context_locked(ctx: *mut UserContext) -> !;
    fn syscall_entry();
    static isr_table: [usize; 256];
}

pub unsafe fn restore_user_context_with_kernel_lock(
    ctx: *mut UserContext,
    kernel_lock: crate::kernel::smp::KernelLockGuard,
) -> ! {
    kernel_lock.defer_unlock_for_user_restore();
    // SAFETY: forwarded to the caller.
    unsafe { restore_user_context_locked(ctx) }
}

#[repr(C, packed)]
struct DescriptorTablePtr {
    limit: u16,
    base: u64,
}

#[repr(C, align(16))]
#[derive(Copy, Clone)]
struct Gdt {
    entries: [u64; 8],
}

#[repr(C, align(16))]
struct Idt {
    entries: [u128; 256],
}

#[repr(C, packed)]
#[derive(Copy, Clone)]
struct Tss {
    reserved0: u32,
    rsp0: u64,
    rsp1: u64,
    rsp2: u64,
    reserved1: u64,
    ist: [u64; 7],
    reserved2: u64,
    reserved3: u16,
    iopb: u16,
}

impl PerCpuInit for Gdt {
    const INIT: Self = Gdt { entries: [0; 8] };
}

impl PerCpuInit for Tss {
    const INIT: Self = Tss {
        reserved0: 0,
        rsp0: 0,
        rsp1: 0,
        rsp2: 0,
        reserved1: 0,
        ist: [0; 7],
        reserved2: 0,
        reserved3: 0,
        iopb: size_of::<Tss>() as u16,
    };
}

/// Descriptor tables. The GDT and TSS are per-core and written only by their
/// own core during its own boot; the IDT is identical on every core, so the
/// first core to arrive fills it in and the rest share it.
static GDTS: PerCpu<Gdt> = PerCpu::new();
static TSSES: PerCpu<Tss> = PerCpu::new();
static IDT: BootOnce<Idt> = BootOnce::new(Idt { entries: [0; 256] });

fn tss_descriptor(base: u64, limit: u32) -> [u64; 2] {
    let low = u64::from(limit & 0xffff)
        | ((base & 0xff_ffff) << 16)
        | (0x89u64 << 40)
        | (u64::from(limit & 0xf_0000) << 32)
        | ((base & 0xff00_0000) << 32);
    [low, base >> 32]
}

fn idt_gate(handler: usize) -> u128 {
    let offset = handler as u64;
    let low = (offset & 0xffff)
        | (u64::from(KERNEL_CS) << 16)
        | (0x8eu64 << 40)
        | ((offset & 0xffff_0000) << 32);
    let high = offset >> 32;
    u128::from(low) | (u128::from(high) << 64)
}

fn reload_kernel_cs() {
    // SAFETY: a far return to the label below, reloading CS from the GDT this
    // core just installed.
    unsafe {
        core::arch::asm!(
            "push {cs}",
            "lea 1f(%rip), %rax",
            "push %rax",
            "lretq",
            "1:",
            cs = in(reg) u64::from(KERNEL_CS),
            out("rax") _,
            options(att_syntax),
        );
    }
}

pub fn install_trap_vector() {
    let core = crate::kernel::smp::current_core_id().min(crate::kernel::smp::MAX_BOOT_CPUS - 1);
    let tss_base = TSSES.slot_ptr(core).expect("core id out of range") as u64;
    let tss_desc = tss_descriptor(tss_base, (size_of::<Tss>() - 1) as u32);
    let kernel_stack_top = crate::arch::x86_64::kernel::current_trap_scratch()
        .map(|scratch| scratch.kernel_stack_top as u64);

    // SAFETY: a core's GDT and TSS are written only by that core, only while
    // it is booting, and no other borrow of them exists.
    unsafe {
        TSSES.with_core_private_mut(|tss| {
            if let Some(rsp0) = kernel_stack_top {
                tss.rsp0 = rsp0;
            }
        });
        GDTS.with_core_private_mut(|gdt| {
            gdt.entries[0] = 0;
            gdt.entries[1] = 0x00af_9a00_0000_ffff;
            gdt.entries[2] = 0x00cf_9200_0000_ffff;
            gdt.entries[3] = 0x00cf_f200_0000_ffff;
            gdt.entries[4] = 0x00af_fa00_0000_ffff;
            gdt.entries[5] = tss_desc[0];
            gdt.entries[6] = tss_desc[1];
        });
    }

    let gdtr = DescriptorTablePtr {
        limit: (size_of::<Gdt>() - 1) as u16,
        base: GDTS.slot_ptr(core).expect("core id out of range") as u64,
    };
    // SAFETY: `gdtr` describes the GDT this core just filled in, whose first
    // entries are the null, kernel code/data, user code/data, and TSS
    // descriptors the selectors below name.
    unsafe {
        registers::lgdt(core::ptr::addr_of!(gdtr) as *const u8);
        reload_kernel_cs();
        registers::load_ds_es_ss(KERNEL_DS);
        registers::ltr(TSS_SEL);
    }

    let idt = IDT.get_or_init(|idt| {
        // SAFETY: `isr_table` is the assembly-provided array of 256 interrupt
        // entry points, in vector order, and it is never written.
        let table = unsafe { &isr_table };
        for (entry, &handler) in idt.entries.iter_mut().zip(table) {
            *entry = idt_gate(handler);
        }
    });
    let idtr = DescriptorTablePtr {
        limit: (size_of::<Idt>() - 1) as u16,
        base: idt as *const Idt as u64,
    };
    // SAFETY: `idtr` describes the fully populated IDT above.
    unsafe {
        registers::lidt(core::ptr::addr_of!(idtr) as *const u8);
    }

    let efer = registers::rdmsr(registers::IA32_EFER) | registers::EFER_SCE;
    // SAFETY: these enable `syscall` and point it at this kernel's entry
    // point with the segment selectors installed above.
    unsafe {
        registers::wrmsr(registers::IA32_EFER, efer);
        registers::wrmsr(
            registers::IA32_STAR,
            (STAR_USER_BASE << 48) | (u64::from(KERNEL_CS) << 32),
        );
        registers::wrmsr(
            registers::IA32_LSTAR,
            syscall_entry as *const () as usize as u64,
        );
        registers::wrmsr(registers::IA32_FMASK, FMASK);
    }
    let _ = (USER_CS, USER_DS);
}

pub fn init_timer() {
    lapic::init();
}

pub fn service_due_timer_interrupts() -> bool {
    if !lapic::timer_irq_pending() {
        return false;
    }
    handle_timer_interrupt();
    true
}

static KERNEL_IRQ_PANIC: AtomicBool = AtomicBool::new(false);

#[unsafe(no_mangle)]
pub extern "C" fn handle_kernel_irq_rust(vector: u64, rip: u64) {
    if vector == u64::from(lapic::IPI_VECTOR) {
        crate::arch::x86_64::smp::ipi::handle_ipi();
        return;
    }
    if vector == u64::from(lapic::TIMER_VECTOR) {
        let _kernel_lock = crate::kernel::smp::KernelLockGuard::lock();
        handle_timer_interrupt();
        return;
    }
    if let Some(irq) = crate::arch::x86_64::machine::ioapic::vector_to_irq(vector) {
        let _kernel_lock = crate::kernel::smp::KernelLockGuard::lock();
        handle_user_irq(irq);
        return;
    }
    if KERNEL_IRQ_PANIC.swap(true, Ordering::SeqCst) {
        crate::arch::x86_64::kernel::boot::halt();
    }
    error!(
        "kernel-mode interrupt vector={:#x} rip={:#x} cr2={:#x}",
        vector,
        rip,
        registers::read_cr2()
    );
    panic!("kernel interrupt");
}

#[unsafe(no_mangle)]
pub extern "C" fn handle_trap_rust(uc: *mut UserContext, cause: u64) -> *mut UserContext {
    if cause == u64::from(lapic::IPI_VECTOR) {
        crate::arch::x86_64::smp::ipi::handle_ipi();
    }
    let kernel_lock = crate::kernel::smp::KernelLockGuard::lock();
    if kernel_lock.remote_stalled_current() {
        return kernel_exit_after_remote_stall(kernel_lock);
    }
    if uc.is_null() {
        panic!("trap entry passed a null user context");
    }
    // SAFETY: trap entry assembly passes the address of the current thread's
    // saved context, which is live for the whole handler.
    let uc = unsafe { &mut *uc };
    uc.restart_pc = uc.pc;
    crate::arch::x86_64::machine::fpu::clear_supervisor_access();

    if cause == VEC_DEVICE_NOT_AVAILABLE {
        if crate::arch::x86_64::machine::fpu::handle_device_not_available(
            crate::object::tcb::current(),
        ) {
            return kernel_exit(uc, kernel_lock);
        }
    }

    if cause == SYSCALL_CAUSE {
        // `syscall` writes RCX with the following instruction. Preempted
        // seL4 invocations must resume at the 2-byte `syscall` itself.
        uc.restart_pc = uc.pc.wrapping_sub(2);
        handle_syscall(uc);
        return kernel_exit(uc, kernel_lock);
    }
    if cause == u64::from(lapic::IPI_VECTOR) {
        return kernel_exit(uc, kernel_lock);
    }
    if cause == u64::from(lapic::TIMER_VECTOR) {
        handle_timer_interrupt();
        return kernel_exit(uc, kernel_lock);
    }
    if let Some(irq) = crate::arch::x86_64::machine::ioapic::vector_to_irq(cause) {
        handle_user_irq(irq);
        return kernel_exit(uc, kernel_lock);
    }

    let cr2 = registers::read_cr2() as u64;
    if !send_fault_ipc(uc, cause, cr2) {
        warn!(
            "user fault: vector={:#x} cr2={:#x} rip={:#x} rsp={:#x}",
            cause,
            cr2,
            uc.pc,
            uc.stack_reg(),
        );
        park_current_thread();
    }
    kernel_exit(uc, kernel_lock)
}

fn fault_message(
    vector: u64,
    cr2: u64,
    uc: &UserContext,
) -> (u64, u64, crate::object::tcb::FaultMrs) {
    let mut mrs = [0; crate::object::tcb::FAULT_IPC_MRS];
    if vector == VEC_PAGE_FAULT {
        mrs[0] = uc.pc;
        mrs[1] = cr2;
        mrs[2] = 0;
        mrs[3] = uc.regs[UserRegister::Rax.index()];
        (FaultLabel::VmFault.raw(), 4, mrs)
    } else {
        mrs[0] = uc.pc;
        mrs[1] = uc.stack_reg();
        mrs[2] = vector;
        mrs[3] = uc.regs[15];
        (FaultLabel::UserException.raw(), 4, mrs)
    }
}

fn write_fault_ipc_message(receiver: TcbRef, badge: u64, label: u64, len: u64, mrs: &[u64]) {
    let info_word = MessageInfo::new(label, 0, 0, len).0;
    receiver.write_fault_ipc_message_regs(badge, info_word, mrs, len);

    let copied_len = len.min(mrs.len() as u64);
    if copied_len > FAULT_MR_REG_COUNT {
        if let Some(buffer) = receiver.ipc_buffer() {
            buffer.set_words(
                1 + FAULT_MR_REG_COUNT as usize,
                &mrs[FAULT_MR_REG_COUNT as usize..copied_len as usize],
            );
        }
    }
}

/// Hand a recorded fault to the handler endpoint: rendezvous with a waiting
/// receiver, or queue the faulting thread as a fault sender.
fn deliver_fault_ipc(
    cur: TcbRef,
    ep: EndpointRef,
    handler_cap: Cap,
    label: u64,
    len: u64,
    mrs: crate::object::tcb::FaultMrs,
) {
    cur.record_fault_message(label, len, mrs);
    let Some(receiver) = ep.pop_receiver() else {
        cur.dequeue();
        cur.set_blocked_fault_sender(
            ep,
            handler_cap.endpoint_badge(),
            handler_cap.endpoint_can_grant(),
            handler_cap.endpoint_can_grant_reply(),
            label,
            len,
            mrs,
        );
        ep.enqueue_waiter(cur, EpState::Sending);
        return;
    };
    write_fault_ipc_message(receiver, handler_cap.endpoint_badge(), label, len, &mrs);
    finish_fault_ipc_receive(receiver, cur, handler_cap, true);
}

fn finish_fault_ipc_receive(
    receiver: TcbRef,
    fault_tcb: TcbRef,
    handler_cap: Cap,
    _reply_rights: bool,
) {
    let receiver_can_grant = receiver.start_receiver_rendezvous();
    if handler_cap.endpoint_can_grant() || handler_cap.endpoint_can_grant_reply() {
        fault_tcb.dequeue();
        if !crate::object::reply::setup_caller_cap(fault_tcb, receiver, receiver_can_grant) {
            fault_tcb.set_inactive();
            fault_tcb.clear_waiting_on();
        }
    } else {
        fault_tcb.set_inactive();
        fault_tcb.clear_waiting_on();
    }
    receiver.finish_receiver_rendezvous();
    receiver.enqueue();
}

fn send_fault_ipc(uc: &mut UserContext, vector: u64, cr2: u64) -> bool {
    use crate::object::tcb;

    let Some(cur) = tcb::current() else {
        return false;
    };
    if vector == VEC_DOUBLE_FAULT {
        return false;
    }
    let _ = VEC_GENERAL_PROTECTION;

    let handler_cap = fault_handler_cap(cur);
    if handler_cap.tag() != Some(CapTag::Endpoint)
        || !handler_cap.endpoint_can_send()
        || !(handler_cap.endpoint_can_grant() || handler_cap.endpoint_can_grant_reply())
    {
        return false;
    }
    let Some(ep) = handler_cap.as_endpoint() else {
        return false;
    };

    let (label, len, mrs) = fault_message(vector, cr2, uc);
    deliver_fault_ipc(cur, ep, handler_cap, label, len, mrs);
    true
}

pub fn send_cap_fault_ipc(uc: &mut UserContext, addr: u64, in_recv_phase: bool) -> bool {
    let mut mrs = [0; crate::object::tcb::FAULT_IPC_MRS];
    mrs[0] = uc.restart_pc;
    mrs[1] = addr;
    mrs[2] = in_recv_phase as u64;
    mrs[3] = 1;
    mrs[4] = 0;
    send_synthetic_fault_ipc(FaultLabel::CapFault.raw(), 5, mrs)
}

fn send_unknown_syscall_fault(uc: &mut UserContext, sysno: isize) -> bool {
    use crate::arch::x86_64::sel4_arch::UNKNOWN_SYSCALL_LENGTH;

    let mut mrs = [0; crate::object::tcb::FAULT_IPC_MRS];
    mrs[0] = uc.regs[2];
    mrs[1] = uc.regs[3];
    mrs[2] = uc.regs[19];
    mrs[3] = uc.regs[8];
    mrs[4] = uc.regs[1];
    mrs[5] = uc.regs[0];
    mrs[6] = uc.regs[4];
    mrs[7] = uc.regs[10];
    mrs[8] = uc.regs[11];
    mrs[9] = uc.regs[9];
    mrs[10] = uc.regs[18];
    mrs[11] = uc.regs[5];
    mrs[12] = uc.regs[6];
    mrs[13] = uc.regs[7];
    mrs[14] = uc.regs[12];
    mrs[15] = uc.pc;
    mrs[16] = uc.stack_reg();
    mrs[17] = uc.regs[13];
    mrs[18] = sysno as u64;
    send_synthetic_fault_ipc(
        FaultLabel::UnknownSyscall.raw(),
        UNKNOWN_SYSCALL_LENGTH,
        mrs,
    )
}

fn send_synthetic_fault_ipc(label: u64, len: u64, mrs: crate::object::tcb::FaultMrs) -> bool {
    use crate::object::tcb;

    let Some(cur) = tcb::current() else {
        return false;
    };
    let handler_cap = fault_handler_cap(cur);
    if handler_cap.tag() != Some(CapTag::Endpoint)
        || !handler_cap.endpoint_can_send()
        || !(handler_cap.endpoint_can_grant() || handler_cap.endpoint_can_grant_reply())
    {
        return false;
    }
    let Some(ep) = handler_cap.as_endpoint() else {
        return false;
    };

    deliver_fault_ipc(cur, ep, handler_cap, label, len, mrs);
    true
}

/// The endpoint cap a thread nominated as its fault handler, resolved in its
/// own CSpace.
fn fault_handler_cap(tcb: TcbRef) -> Cap {
    let cptr = tcb.fault_endpoint_cptr();
    if cptr == 0 {
        return Cap::null();
    }
    match cspace::lookup_cap_in(tcb.cspace_cap(), cptr, cspace::WORD_BITS) {
        Ok((cap, _)) => cap,
        Err(_) => Cap::null(),
    }
}

fn handle_timer_interrupt() {
    if let Some(cur) = crate::object::tcb::current() {
        crate::object::tcb::timer_tick(cur);
    }
    handle_user_irq(irq::KERNEL_TIMER_IRQ as u64);
    lapic::eoi();
}

fn handle_user_irq(irq: u64) {
    if !crate::object::irq::signal_irq(irq) {
        irq::complete_external_irq(irq);
    }
}

pub fn idle_scheduler_loop() -> ! {
    loop {
        let next_context = {
            let kernel_lock = crate::kernel::smp::KernelLockGuard::lock();
            let _ = service_due_timer_interrupts();
            match crate::object::tcb::schedule() {
                None => {
                    switch_to_idle_thread_if_needed();
                    switch_to_kernel_vspace();
                    None
                }
                Some(next) => {
                    crate::object::tcb::set_current(Some(next));
                    let ctx = next.prepare_for_user_restore();
                    switch_to_tcb_vspace(next);
                    Some((ctx, kernel_lock))
                }
            }
        };
        if let Some((ctx, kernel_lock)) = next_context {
            kernel_lock.defer_unlock_for_user_restore();
            // SAFETY: the context belongs to the thread just picked, and the kernel lock
            // is handed to the restore path.
            unsafe { restore_user_context_locked(ctx) };
        }
        unsafe {
            core::arch::asm!("sti; hlt; cli", options(nomem, nostack));
        }
    }
}

fn switch_to_kernel_vspace() {
    let Some(kernel_root) = crate::kernel::smp::kernel_vspace_root() else {
        return;
    };
    if vspace::current_cr3() != kernel_root {
        // SAFETY: the published kernel root maps the kernel image and PSpace window.
        unsafe { vspace::switch_cr3(kernel_root) };
    }
}

fn switch_to_tcb_vspace(tcb: TcbRef) {
    let vroot = tcb.vspace_cap();
    if vroot.tag() != Some(CapTag::PageTable) {
        return;
    }
    let root_kva = vroot.page_table_base_ptr();
    if root_kva == 0 {
        return;
    }
    let asid = vroot.page_table_mapped_asid();
    if !vroot.page_table_is_mapped() || asid == 0 {
        return;
    }
    if crate::object::asid::lookup(asid) != root_kva {
        return;
    }
    let new_cr3 = vspace::cr3_from_kva(root_kva, asid as u64);
    if new_cr3 == 0 {
        return;
    }
    if vspace::current_cr3() != new_cr3 {
        // SAFETY: the checks above proved this root page table is the one the ASID
        // table resolves for the thread's VSpace, and every VSpace shares the
        // kernel window.
        unsafe { vspace::switch_cr3(new_cr3) };
    }
}

#[inline]
fn kernel_exit(
    uc: &mut UserContext,
    kernel_lock: crate::kernel::smp::KernelLockGuard,
) -> *mut UserContext {
    use crate::object::tcb;
    let cur = tcb::current();

    loop {
        if let Some(cur) = cur {
            cur.enqueue_if_migrated_from_current_core();
            if tcb::take_continue_current_once(Some(cur)) && cur.is_runnable_on_current_core() {
                cur.prepare_for_user_restore();
                return finish_kernel_exit(uc as *mut UserContext, kernel_lock);
            }
            let rotate = tcb::take_reschedule_required();
            if cur.is_runnable_on_current_core() && !rotate {
                cur.prepare_for_user_restore();
                return finish_kernel_exit(uc as *mut UserContext, kernel_lock);
            }
            if cur.is_runnable_on_current_core() {
                cur.enqueue();
            }
        }

        if let Some(next) = tcb::schedule() {
            if Some(next) != cur {
                tcb::set_current(Some(next));
                let ctx = next.prepare_for_user_restore();
                switch_to_tcb_vspace(next);
                return finish_kernel_exit(ctx, kernel_lock);
            }
            if next.is_runnable_on_current_core() {
                next.prepare_for_user_restore();
                return finish_kernel_exit(uc as *mut UserContext, kernel_lock);
            }
            continue;
        }

        // schedule() found nothing. Safe to fall through *only* if current is
        // still runnable — otherwise we'd resume a blocked TCB's user mode and
        // break IPC semantics.
        if cur.is_some_and(TcbRef::is_runnable_on_current_core) {
            let cur = cur.expect("just checked");
            cur.prepare_for_user_restore();
            return finish_kernel_exit(uc as *mut UserContext, kernel_lock);
        }

        switch_to_idle_thread_if_needed();
        switch_to_kernel_vspace();
        drop(kernel_lock);
        idle_scheduler_loop();
    }
}

/// Park this core on its idle thread unless it is already there.
fn switch_to_idle_thread_if_needed() {
    use crate::object::tcb;
    if !tcb::current().is_some_and(tcb::is_idle_thread) {
        tcb::switch_to_idle_thread();
    }
}

fn kernel_exit_after_remote_stall(
    kernel_lock: crate::kernel::smp::KernelLockGuard,
) -> *mut UserContext {
    use crate::object::tcb;

    loop {
        if let Some(next) = tcb::schedule() {
            tcb::set_current(Some(next));
            let ctx = next.prepare_for_user_restore();
            switch_to_tcb_vspace(next);
            return finish_kernel_exit(ctx, kernel_lock);
        }

        switch_to_idle_thread_if_needed();
        switch_to_kernel_vspace();
        drop(kernel_lock);
        idle_scheduler_loop();
    }
}

#[inline]
fn finish_kernel_exit(
    ctx: *mut UserContext,
    kernel_lock: crate::kernel::smp::KernelLockGuard,
) -> *mut UserContext {
    kernel_lock.defer_unlock_for_user_restore();
    ctx
}

fn park_current_thread() -> ! {
    loop {
        // SAFETY: halting this core.
        unsafe { core::arch::asm!("hlt", options(nomem, nostack)) };
    }
}

fn debug_halt(message: &str) -> ! {
    error!("{message}");
    crate::arch::x86_64::kernel::boot::halt()
}

fn handle_debug_name_thread(uc: &UserContext) {
    let cptr = uc.cap_reg();
    let Ok((cap, _)) = crate::api::cspace::lookup_cap_current(cptr) else {
        debug_halt("SysDebugNameThread: cap is not a TCB, halting");
    };
    let Some(target) = crate::object::tcb::from_cap(cap) else {
        debug_halt("SysDebugNameThread: cap is not a TCB, halting");
    };

    let Some(buffer) = crate::object::tcb::current().and_then(TcbRef::ipc_buffer) else {
        debug_halt("SysDebugNameThread: Failed to lookup IPC buffer, halting");
    };

    // The name follows the message-info word, as a NUL-terminated string
    // packed into the message registers.
    let mut name = [0u8; N_TOTAL_MSG_REGISTERS * WORD_BYTES];
    for (i, word) in name.chunks_mut(WORD_BYTES).enumerate() {
        word.copy_from_slice(&buffer.word(1 + i).to_le_bytes());
    }
    match name.iter().position(|&byte| byte == 0) {
        Some(len) => target.set_debug_name(&name[..len]),
        None => debug_halt("SysDebugNameThread: Name too long, halting"),
    }
}

fn handle_syscall(uc: &mut UserContext) {
    // Our sel4-user/linux-compat path puts the number in RAX. Upstream libsel4
    // x86_64 uses RDX. Prefer a recognised seL4 number in either register, then
    // fall back to RAX so positive Linux syscalls become UnknownSyscall.
    let rax = uc.syscall_reg() as isize;
    let rdx = uc.libsel4_syscall_reg() as isize;
    let raw_sysno = if SyscallNumber::from_raw(rax).is_some() {
        rax
    } else if SyscallNumber::from_raw(rdx).is_some() {
        rdx
    } else {
        rax
    };

    match SyscallNumber::from_raw(raw_sysno) {
        Some(SyscallNumber::DebugPutChar) => {
            let ch = uc.cap_reg() as u8;
            crate::machine::console::putc(ch);
        }
        Some(SyscallNumber::DebugNameThread) => {
            handle_debug_name_thread(uc);
        }
        Some(SyscallNumber::DebugCapIdentify) => {
            let cptr = uc.cap_reg();
            let tag = match crate::api::cspace::lookup_cap_current(cptr) {
                Ok((cap, _)) => cap.tag_raw(),
                Err(_) => 0,
            };
            // libsel4 x86_64 reads the result from RDI (capRegister), not RAX.
            uc.set_cap_reg(tag);
        }
        Some(SyscallNumber::DebugHalt) => {
            debug_halt("Debug halt syscall from user thread");
        }
        Some(SyscallNumber::DebugSendIpi) => {
            debug_halt("SysDebugSendIPI: not supported on this architecture");
        }
        Some(SyscallNumber::SetTLSBase) => {
            uc.set_tls_reg(uc.cap_reg());
        }
        Some(SyscallNumber::DebugDumpScheduler | SyscallNumber::DebugSnapshot) => {}
        Some(SyscallNumber::Yield) => {
            if let Some(cur) = crate::object::tcb::current() {
                crate::object::tcb::yield_current(cur);
            }
        }
        Some(SyscallNumber::Call) => {
            crate::api::syscall::do_call(uc);
        }
        Some(SyscallNumber::Send) => {
            crate::api::syscall::do_send(uc, false);
        }
        Some(SyscallNumber::NonBlockingSend) => {
            crate::api::syscall::do_send(uc, true);
        }
        Some(SyscallNumber::Reply) => {
            crate::api::ipc::reply(uc);
        }
        Some(SyscallNumber::Recv | SyscallNumber::NonBlockingRecv) => {
            let blocking = SyscallNumber::from_raw(raw_sysno) == Some(SyscallNumber::Recv);
            crate::api::syscall::do_recv(uc, blocking);
        }
        Some(SyscallNumber::ReplyRecv) => {
            crate::api::ipc::reply_recv(uc);
        }
        None => {
            if !send_unknown_syscall_fault(uc, raw_sysno) {
                warn!(
                    "unknown syscall number {} (regs: rdi={:#x} rsi={:#x} rax={:#x})",
                    raw_sysno,
                    uc.cap_reg(),
                    uc.msg_info(),
                    uc.syscall_reg(),
                );
                park_current_thread();
            }
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn kernel_trap_panic() -> ! {
    error!(
        "kernel-mode trap: cr2={:#x} cr3={:#x}",
        registers::read_cr2(),
        registers::read_cr3()
    );
    panic!("kernel trap");
}
