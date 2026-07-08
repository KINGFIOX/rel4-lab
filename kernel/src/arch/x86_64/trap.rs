use crate::object::cap::Cap;

pub const X86_64_NUM_FP_REGS: usize = 16;

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct FpuState {
    pub regs: [u128; X86_64_NUM_FP_REGS],
    pub mxcsr: u32,
    pub _pad: [u32; 3],
}

impl FpuState {
    pub const fn zero() -> Self {
        Self {
            regs: [0; X86_64_NUM_FP_REGS],
            mxcsr: 0,
            _pad: [0; 3],
        }
    }
}

#[repr(C)]
#[derive(Default)]
pub struct UserContext {
    pub regs: [u64; 32],
    pub pc: u64,
    pub sstatus: u64,
    pub restart_pc: u64,
    pub fpu: FpuState,
}

impl UserContext {
    pub const fn zero() -> Self {
        Self {
            regs: [0; 32],
            pc: 0,
            sstatus: 0,
            restart_pc: 0,
            fpu: FpuState::zero(),
        }
    }
}

#[repr(usize)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum UserRegister {
    Ra = 1,
    Sp = 7,
    Gp = 0,
    Tp = 6,
    T0 = 10,
    A0 = 15,
    A1 = 14,
    A2 = 13,
    A3 = 12,
    A4 = 8,
    A5 = 9,
    A6 = 4,
    A7 = 5,
}

impl UserRegister {
    pub const fn index(self) -> usize {
        self as usize
    }
}

pub const SEL4_USER_CONTEXT_WORDS: usize = 32;

pub const SEL4_USER_CONTEXT_REGS: [usize; SEL4_USER_CONTEXT_WORDS] = [
    0,
    UserRegister::Sp.index(),
    0,
    0,
    0,
    0,
    0,
    UserRegister::A0.index(),
    UserRegister::A1.index(),
    UserRegister::A2.index(),
    UserRegister::A3.index(),
    UserRegister::A4.index(),
    UserRegister::A5.index(),
    UserRegister::A6.index(),
    UserRegister::A7.index(),
    UserRegister::Ra.index(),
    UserRegister::Tp.index(),
    UserRegister::T0.index(),
    11,
    16,
    17,
    18,
    19,
    20,
    21,
    22,
    23,
    24,
    25,
    26,
    27,
    28,
];

pub const SEL4_TCB_FRAME_REGS: [usize; 16] = [
    SEL4_USER_CONTEXT_REGS[0],
    SEL4_USER_CONTEXT_REGS[1],
    SEL4_USER_CONTEXT_REGS[2],
    SEL4_USER_CONTEXT_REGS[3],
    SEL4_USER_CONTEXT_REGS[4],
    SEL4_USER_CONTEXT_REGS[5],
    SEL4_USER_CONTEXT_REGS[6],
    SEL4_USER_CONTEXT_REGS[7],
    SEL4_USER_CONTEXT_REGS[8],
    SEL4_USER_CONTEXT_REGS[9],
    SEL4_USER_CONTEXT_REGS[10],
    SEL4_USER_CONTEXT_REGS[11],
    SEL4_USER_CONTEXT_REGS[12],
    SEL4_USER_CONTEXT_REGS[13],
    SEL4_USER_CONTEXT_REGS[14],
    SEL4_USER_CONTEXT_REGS[15],
];

pub const SEL4_TCB_GP_REGS: [usize; 16] = [
    SEL4_USER_CONTEXT_REGS[16],
    SEL4_USER_CONTEXT_REGS[17],
    SEL4_USER_CONTEXT_REGS[18],
    SEL4_USER_CONTEXT_REGS[19],
    SEL4_USER_CONTEXT_REGS[20],
    SEL4_USER_CONTEXT_REGS[21],
    SEL4_USER_CONTEXT_REGS[22],
    SEL4_USER_CONTEXT_REGS[23],
    SEL4_USER_CONTEXT_REGS[24],
    SEL4_USER_CONTEXT_REGS[25],
    SEL4_USER_CONTEXT_REGS[26],
    SEL4_USER_CONTEXT_REGS[27],
    SEL4_USER_CONTEXT_REGS[28],
    SEL4_USER_CONTEXT_REGS[29],
    SEL4_USER_CONTEXT_REGS[30],
    SEL4_USER_CONTEXT_REGS[31],
];

pub const SSTATUS_FS_MASK: u64 = 0;
pub const SSTATUS_FS_CLEAN: u64 = 0;
pub const USER_SSTATUS: u64 = 0x202;
pub const ROOTSERVER_SSTATUS: u64 = USER_SSTATUS;

pub unsafe fn restore_user_context(_ctx: *mut UserContext) -> ! {
    crate::arch::current::boot::halt()
}

pub unsafe fn restore_user_context_with_kernel_lock(
    ctx: *mut UserContext,
    kernel_lock: crate::kernel::smp::KernelLockGuard,
) -> ! {
    kernel_lock.defer_unlock_for_user_restore();
    unsafe { restore_user_context(ctx) }
}

pub fn install_trap_vector() {}

pub fn init_timer() {}

pub fn service_due_timer_interrupts() -> bool {
    false
}

pub fn idle_scheduler_loop() -> ! {
    crate::arch::current::boot::halt()
}

pub fn send_cap_fault_ipc(_uc: &mut UserContext, _addr: u64, _in_recv_phase: bool) -> bool {
    false
}

pub fn same_object_as(_left: Cap, _right: Cap) -> bool {
    false
}
