//! x86_64 seL4 ABI surface. Register indices follow upstream
//! `arch/x86/arch/64/mode/machine/registerset.h`.

pub mod invocation;
pub mod object_type;

pub use invocation::ArchInvocation;
pub use object_type::ObjectType;

use crate::object::cap::Cap;

pub const X86_64_NUM_FP_REGS: usize = 16;
pub const SEL4_USER_CONTEXT_WORDS: usize = 24;

const RDI: usize = 0;
const RSI: usize = 1;
const RAX: usize = 2;
const RSP: usize = 16;
const FAULT_IP: usize = 17;
const NEXT_IP: usize = 14;
const FLAGS: usize = 13;
const R10: usize = 9;
const R8: usize = 10;
const R9: usize = 11;
const R15: usize = 12;
const FS_BASE: usize = 22;

/// User-visible TCB register ABI indices for Read/WriteRegisters.
/// Index 0 is the FaultIP sentinel used by shared TCB code.
pub const SEL4_USER_CONTEXT_REGS: [usize; 32] = [
    FAULT_IP, RSP, FLAGS, RAX, 3, 4, 5, 6, 7, R8, R9, R10, 18, 19, R15, RSI, RDI, FS_BASE, 23, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];
pub const SEL4_TCB_FRAME_REGS: [usize; 16] = [
    FAULT_IP, RSP, FLAGS, RAX, 3, 4, 5, 6, 7, R8, R9, R10, 18, 19, R15, RSI,
];
pub const SEL4_TCB_GP_REGS: [usize; 16] = [
    FS_BASE, 23, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];

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

/// seL4 x86_64 user context. `regs` is the kernel trap-save array, not the
/// compact libsel4 `seL4_UserContext` struct. Named field aliases keep the
/// shared TCB/boot paths from talking about `sstatus`.
#[repr(C)]
#[derive(Default)]
pub struct UserContext {
    pub regs: [u64; SEL4_USER_CONTEXT_WORDS],
    pub pc: u64,
    pub restart_pc: u64,
    pub fpu: FpuState,
}

impl UserContext {
    pub const fn zero() -> Self {
        Self {
            regs: [0; SEL4_USER_CONTEXT_WORDS],
            pc: 0,
            restart_pc: 0,
            fpu: FpuState::zero(),
        }
    }

    pub fn cap_reg(&self) -> u64 {
        self.regs[RDI]
    }

    pub fn set_cap_reg(&mut self, value: u64) {
        self.regs[RDI] = value;
    }

    pub fn msg_info(&self) -> u64 {
        self.regs[RSI]
    }

    pub fn set_msg_info(&mut self, value: u64) {
        self.regs[RSI] = value;
    }

    pub fn mr(&self, index: usize) -> u64 {
        match index {
            0 => self.regs[R10],
            1 => self.regs[R8],
            2 => self.regs[R9],
            3 => self.regs[R15],
            _ => 0,
        }
    }

    pub fn set_mr(&mut self, index: usize, value: u64) {
        let slot = match index {
            0 => R10,
            1 => R8,
            2 => R9,
            3 => R15,
            _ => return,
        };
        self.regs[slot] = value;
    }

    pub fn reply_reg(&self) -> u64 {
        self.regs[5]
    }

    pub fn set_reply_reg(&mut self, value: u64) {
        self.regs[5] = value;
    }

    pub fn syscall_reg(&self) -> u64 {
        self.regs[RAX]
    }

    pub fn stack_reg(&self) -> u64 {
        self.regs[RSP]
    }

    pub fn set_stack_reg(&mut self, value: u64) {
        self.regs[RSP] = value;
    }

    pub fn return_reg(&self) -> u64 {
        self.regs[RAX]
    }

    pub fn set_return_reg(&mut self, value: u64) {
        self.regs[RAX] = value;
    }

    pub fn tls_reg(&self) -> u64 {
        self.regs[FS_BASE]
    }

    pub fn set_tls_reg(&mut self, value: u64) {
        self.regs[FS_BASE] = value;
    }

    pub fn scratch_reg(&self) -> u64 {
        self.regs[RAX]
    }

    pub fn clear_ipc_regs(&mut self) {
        self.set_cap_reg(0);
        self.set_msg_info(0);
        self.set_mr(0, 0);
        self.set_mr(1, 0);
        self.set_mr(2, 0);
        self.set_mr(3, 0);
    }
}

#[repr(usize)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum UserRegister {
    Ra = RAX,
    Sp = RSP,
    Gp = RDI,
    Tp = FS_BASE,
    T0 = RAX,
    A0 = RDI,
    A1 = RSI,
    A2 = R10,
    A3 = R8,
    A4 = R9,
    A5 = R15,
    A6 = 5,
    A7 = RAX,
}

impl UserRegister {
    pub const fn index(self) -> usize {
        self as usize
    }
}

pub fn init_user_context(context: &mut UserContext) {
    context.regs[FLAGS] = 0x202;
}

pub fn init_rootserver_context(context: &mut UserContext, entry: u64, stack: u64, bootinfo: u64) {
    context.pc = entry;
    context.restart_pc = entry;
    context.regs[NEXT_IP] = entry;
    context.regs[FAULT_IP] = entry;
    context.regs[FLAGS] = 0x202;
    context.set_cap_reg(bootinfo);
    context.set_msg_info(0);
    context.set_stack_reg(stack);
}

pub fn set_fpu_context_enabled(_context: &mut UserContext, _enabled: bool) {}

pub fn same_object_as(_left: Cap, _right: Cap) -> bool {
    false
}
