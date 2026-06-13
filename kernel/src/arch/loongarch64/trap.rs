//! LoongArch64 user-context ABI skeleton.
//!
//! This module fixes the kernel-visible register naming and TCB register
//! ordering, and provides the first executable user-restore path plus an early
//! diagnostic trap vector. Syscall/fault dispatch is still staging work.

use core::arch::global_asm;

use log_crate::error;

use crate::arch::loongarch64::csr::{self, CSR_BADV, CSR_ERA, CSR_ESTAT, CSR_PRMD};

/// User-mode register snapshot shape for the future LoongArch64 trap entry.
///
/// `regs[0]` is hardwired zero. The remaining indexes are architectural GPR
/// numbers, matching the LoongArch psABI register names.
#[repr(C)]
#[derive(Default)]
pub struct UserContext {
    pub regs: [u64; 32],
    pub pc: u64,
    pub sstatus: u64,
    pub restart_pc: u64,
}

const _: () = {
    assert!(core::mem::size_of::<UserContext>() == 35 * 8);
    assert!(core::mem::offset_of!(UserContext, regs) == 0);
    assert!(core::mem::offset_of!(UserContext, pc) == 32 * 8);
    assert!(core::mem::offset_of!(UserContext, sstatus) == 33 * 8);
    assert!(core::mem::offset_of!(UserContext, restart_pc) == 34 * 8);
};

impl UserContext {
    pub const fn zero() -> Self {
        Self {
            regs: [0; 32],
            pc: 0,
            sstatus: 0,
            restart_pc: 0,
        }
    }
}

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct TrapRecord {
    pub era: u64,
    pub prmd: u64,
    pub estat: u64,
    pub badv: u64,
}

impl TrapRecord {
    pub const fn zero() -> Self {
        Self {
            era: 0,
            prmd: 0,
            estat: 0,
            badv: 0,
        }
    }
}

pub const LOONGARCH64_TRAP_STACK_SIZE: usize = 16 * 1024;

#[repr(align(16))]
pub struct TrapStack(pub [u8; LOONGARCH64_TRAP_STACK_SIZE]);

#[unsafe(no_mangle)]
static mut LOONGARCH64_TRAP_CONTEXT: UserContext = UserContext::zero();

#[unsafe(no_mangle)]
static mut LOONGARCH64_TRAP_RECORD: TrapRecord = TrapRecord::zero();

#[unsafe(no_mangle)]
static mut LOONGARCH64_TRAP_STACK: TrapStack = TrapStack([0; LOONGARCH64_TRAP_STACK_SIZE]);

/// Register name -> index in `UserContext.regs`.
#[repr(usize)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum UserRegister {
    Ra = 1,
    Tp = 2,
    Sp = 3,
    A0 = 4,
    A1 = 5,
    A2 = 6,
    A3 = 7,
    A4 = 8,
    A5 = 9,
    A6 = 10,
    A7 = 11,
    T0 = 12,
    T1 = 13,
    T2 = 14,
    T3 = 15,
    T4 = 16,
    T5 = 17,
    T6 = 18,
    T7 = 19,
    T8 = 20,
    R21 = 21,
    Fp = 22,
    S0 = 23,
    S1 = 24,
    S2 = 25,
    S3 = 26,
    S4 = 27,
    S5 = 28,
    S6 = 29,
    S7 = 30,
    S8 = 31,
}

impl UserRegister {
    pub const fn index(self) -> usize {
        self as usize
    }
}

/// LoongArch64 frame registers for seL4-style TCB copy operations.
///
/// Until an upstream seL4 LoongArch port is vendored locally, the project ABI
/// is `pc` followed by GPR r1..r31. The frame/integer split preserves the
/// existing 16 + 16 seL4 TCB register-operation shape.
pub const SEL4_TCB_FRAME_REGS: [usize; 16] = [
    0,
    UserRegister::Ra.index(),
    UserRegister::Tp.index(),
    UserRegister::Sp.index(),
    UserRegister::A0.index(),
    UserRegister::A1.index(),
    UserRegister::A2.index(),
    UserRegister::A3.index(),
    UserRegister::A4.index(),
    UserRegister::A5.index(),
    UserRegister::A6.index(),
    UserRegister::A7.index(),
    UserRegister::T0.index(),
    UserRegister::T1.index(),
    UserRegister::T2.index(),
    UserRegister::T3.index(),
];

/// LoongArch64 integer registers for seL4-style TCB copy operations.
pub const SEL4_TCB_GP_REGS: [usize; 16] = [
    UserRegister::T4.index(),
    UserRegister::T5.index(),
    UserRegister::T6.index(),
    UserRegister::T7.index(),
    UserRegister::T8.index(),
    UserRegister::R21.index(),
    UserRegister::Fp.index(),
    UserRegister::S0.index(),
    UserRegister::S1.index(),
    UserRegister::S2.index(),
    UserRegister::S3.index(),
    UserRegister::S4.index(),
    UserRegister::S5.index(),
    UserRegister::S6.index(),
    UserRegister::S7.index(),
    UserRegister::S8.index(),
];

pub const SEL4_USER_CONTEXT_WORDS: usize = SEL4_TCB_FRAME_REGS.len() + SEL4_TCB_GP_REGS.len();

/// LoongArch64 `seL4_UserContext` word index to local GPR index.
///
/// Index 0 is the PC sentinel; indexes 1..31 are GPR r1..r31.
pub const SEL4_USER_CONTEXT_REGS: [usize; SEL4_USER_CONTEXT_WORDS] = [
    0,
    UserRegister::Ra.index(),
    UserRegister::Tp.index(),
    UserRegister::Sp.index(),
    UserRegister::A0.index(),
    UserRegister::A1.index(),
    UserRegister::A2.index(),
    UserRegister::A3.index(),
    UserRegister::A4.index(),
    UserRegister::A5.index(),
    UserRegister::A6.index(),
    UserRegister::A7.index(),
    UserRegister::T0.index(),
    UserRegister::T1.index(),
    UserRegister::T2.index(),
    UserRegister::T3.index(),
    UserRegister::T4.index(),
    UserRegister::T5.index(),
    UserRegister::T6.index(),
    UserRegister::T7.index(),
    UserRegister::T8.index(),
    UserRegister::R21.index(),
    UserRegister::Fp.index(),
    UserRegister::S0.index(),
    UserRegister::S1.index(),
    UserRegister::S2.index(),
    UserRegister::S3.index(),
    UserRegister::S4.index(),
    UserRegister::S5.index(),
    UserRegister::S6.index(),
    UserRegister::S7.index(),
    UserRegister::S8.index(),
];

pub const SSTATUS_FS_MASK: u64 = 0;
pub const SSTATUS_FS_CLEAN: u64 = 0;
pub const PRMD_PPLV_USER: u64 = 0b11;
pub const PRMD_PIE: u64 = 1 << 2;
pub const USER_SSTATUS: u64 = PRMD_PPLV_USER | PRMD_PIE;
pub const ROOTSERVER_SSTATUS: u64 = USER_SSTATUS;

global_asm!(
    r#"
    .section .text.traps
    .align 6

    .globl trap_entry
trap_entry:
    la.local $t0, {trap_context}
    st.d    $zero, $t0,  0*8
    st.d    $ra,   $t0,  1*8
    st.d    $tp,   $t0,  2*8
    st.d    $sp,   $t0,  3*8
    st.d    $a0,   $t0,  4*8
    st.d    $a1,   $t0,  5*8
    st.d    $a2,   $t0,  6*8
    st.d    $a3,   $t0,  7*8
    st.d    $a4,   $t0,  8*8
    st.d    $a5,   $t0,  9*8
    st.d    $a6,   $t0, 10*8
    st.d    $a7,   $t0, 11*8
    st.d    $t0,   $t0, 12*8
    st.d    $t1,   $t0, 13*8
    st.d    $t2,   $t0, 14*8
    st.d    $t3,   $t0, 15*8
    st.d    $t4,   $t0, 16*8
    st.d    $t5,   $t0, 17*8
    st.d    $t6,   $t0, 18*8
    st.d    $t7,   $t0, 19*8
    st.d    $t8,   $t0, 20*8
    st.d    $r21,  $t0, 21*8
    st.d    $fp,   $t0, 22*8
    st.d    $s0,   $t0, 23*8
    st.d    $s1,   $t0, 24*8
    st.d    $s2,   $t0, 25*8
    st.d    $s3,   $t0, 26*8
    st.d    $s4,   $t0, 27*8
    st.d    $s5,   $t0, 28*8
    st.d    $s6,   $t0, 29*8
    st.d    $s7,   $t0, 30*8
    st.d    $s8,   $t0, 31*8

    csrrd   $t1, {csr_era}
    st.d    $t1, $t0, 32*8
    st.d    $t1, $t0, 34*8
    csrrd   $t1, {csr_prmd}
    st.d    $t1, $t0, 33*8

    la.local $t1, {trap_record}
    csrrd   $t2, {csr_era}
    st.d    $t2, $t1, 0*8
    csrrd   $t2, {csr_prmd}
    st.d    $t2, $t1, 1*8
    csrrd   $t2, {csr_estat}
    st.d    $t2, $t1, 2*8
    csrrd   $t2, {csr_badv}
    st.d    $t2, $t1, 3*8

    la.local $sp, {trap_stack}
    li.d    $t1, {trap_stack_size}
    add.d   $sp, $sp, $t1
    addi.d  $sp, $sp, -16

    la.local $a0, {trap_context}
    bl      {handle_trap_rust}

1:
    idle    0
    b       1b

    .globl restore_user_context_locked
restore_user_context_locked:
    addi.d  $sp, $sp, -16
    st.d    $a0, $sp, 0
    bl      kernel_unlock_for_user_restore
    ld.d    $a0, $sp, 0
    addi.d  $sp, $sp, 16
    b       restore_user_context

    .globl restore_user_context
restore_user_context:
    move    $t0, $a0

    ld.d    $t1, $t0, 32*8
    csrwr   $t1, {csr_era}
    ld.d    $t1, $t0, 33*8
    csrwr   $t1, {csr_prmd}

    ld.d    $ra, $t0,  1*8
    ld.d    $tp, $t0,  2*8
    ld.d    $sp, $t0,  3*8
    ld.d    $a0, $t0,  4*8
    ld.d    $a1, $t0,  5*8
    ld.d    $a2, $t0,  6*8
    ld.d    $a3, $t0,  7*8
    ld.d    $a4, $t0,  8*8
    ld.d    $a5, $t0,  9*8
    ld.d    $a6, $t0, 10*8
    ld.d    $a7, $t0, 11*8
    ld.d    $t1, $t0, 13*8
    ld.d    $t2, $t0, 14*8
    ld.d    $t3, $t0, 15*8
    ld.d    $t4, $t0, 16*8
    ld.d    $t5, $t0, 17*8
    ld.d    $t6, $t0, 18*8
    ld.d    $t7, $t0, 19*8
    ld.d    $t8, $t0, 20*8
    ld.d    $r21, $t0, 21*8
    ld.d    $fp, $t0, 22*8
    ld.d    $s0, $t0, 23*8
    ld.d    $s1, $t0, 24*8
    ld.d    $s2, $t0, 25*8
    ld.d    $s3, $t0, 26*8
    ld.d    $s4, $t0, 27*8
    ld.d    $s5, $t0, 28*8
    ld.d    $s6, $t0, 29*8
    ld.d    $s7, $t0, 30*8
    ld.d    $s8, $t0, 31*8
    ld.d    $t0, $t0, 12*8

    ertn
"#,
    trap_context = sym LOONGARCH64_TRAP_CONTEXT,
    trap_record = sym LOONGARCH64_TRAP_RECORD,
    trap_stack = sym LOONGARCH64_TRAP_STACK,
    trap_stack_size = const LOONGARCH64_TRAP_STACK_SIZE,
    handle_trap_rust = sym handle_trap_rust,
    csr_era = const CSR_ERA,
    csr_prmd = const CSR_PRMD,
    csr_estat = const CSR_ESTAT,
    csr_badv = const CSR_BADV,
);

unsafe extern "C" {
    pub fn trap_entry();
    pub fn restore_user_context(ctx: *mut UserContext) -> !;
    fn restore_user_context_locked(ctx: *mut UserContext) -> !;
}

#[unsafe(no_mangle)]
pub extern "C" fn handle_trap_rust(uc: *mut UserContext) -> ! {
    let record = unsafe { LOONGARCH64_TRAP_RECORD };
    let (pc, a0, a1, a2, a3, a7) = if uc.is_null() {
        (0, 0, 0, 0, 0, 0)
    } else {
        let uc = unsafe { &*uc };
        (
            uc.pc,
            uc.regs[UserRegister::A0.index()],
            uc.regs[UserRegister::A1.index()],
            uc.regs[UserRegister::A2.index()],
            uc.regs[UserRegister::A3.index()],
            uc.regs[UserRegister::A7.index()],
        )
    };

    error!(
        "loongarch64 trap: pc={:#x} era={:#x} prmd={:#x} estat={:#x} badv={:#x} a0={:#x} a1={:#x} a2={:#x} a3={:#x} a7={:#x}",
        pc, record.era, record.prmd, record.estat, record.badv, a0, a1, a2, a3, a7
    );
    crate::arch::loongarch64::boot::halt()
}

pub fn install_trap_vector() {
    let addr = trap_entry as *const () as usize;
    debug_assert!(addr & 0x3f == 0, "eentry must be 64-byte aligned");
    csr::set_eentry(addr);
    csr::ibar();
}

pub fn init_timer() {}

pub fn service_due_timer_interrupts() -> bool {
    false
}

pub unsafe fn restore_user_context_with_kernel_lock(
    ctx: *mut UserContext,
    kernel_lock: crate::kernel::smp::KernelLockGuard,
) -> ! {
    kernel_lock.defer_unlock_for_user_restore();
    unsafe { restore_user_context_locked(ctx) }
}

pub fn idle_scheduler_loop() -> ! {
    loop {}
}

pub fn send_cap_fault_ipc(_uc: &mut UserContext, _addr: u64, _in_recv_phase: bool) -> bool {
    false
}

pub fn send_timeout_fault_ipc_for<T>(_fault_tcb: *mut T) -> bool {
    false
}
