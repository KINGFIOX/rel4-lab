//! x86_64 seL4 ABI surface. Register indices follow upstream
//! `arch/x86/arch/64/mode/machine/registerset.h`.

pub mod invocation;
pub mod object_type;

pub use object_type::ObjectType;

/// Opaque address-space root programmed by `switch_vspace`.
pub type VspaceRoot = u64;

use crate::object::cap::{Cap, CapTag};

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
    UserRegister::Rip.index(),
    UserRegister::Rsp.index(),
    UserRegister::Rflags.index(),
    UserRegister::Rax.index(),
    3,
    4,
    5,
    6,
    7,
    UserRegister::R8.index(),
    UserRegister::R9.index(),
    UserRegister::R10.index(),
    18,
    19,
    UserRegister::R15.index(),
    UserRegister::Rsi.index(),
    UserRegister::Rdi.index(),
    UserRegister::FsBase.index(),
    23,
    0,
    0,
    0,
    0,
    0,
    0,
    0,
    0,
    0,
    0,
    0,
    0,
    0,
];
pub const SEL4_TCB_FRAME_REGS: [usize; 16] = [
    UserRegister::Rip.index(),
    UserRegister::Rsp.index(),
    UserRegister::Rflags.index(),
    UserRegister::Rax.index(),
    3,
    4,
    5,
    6,
    7,
    UserRegister::R8.index(),
    UserRegister::R9.index(),
    UserRegister::R10.index(),
    18,
    19,
    UserRegister::R15.index(),
    UserRegister::Rsi.index(),
];
pub const SEL4_TCB_GP_REGS: [usize; 16] = [
    UserRegister::FsBase.index(),
    23,
    0,
    0,
    0,
    0,
    0,
    0,
    0,
    0,
    0,
    0,
    0,
    0,
    0,
    0,
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
/// compact libsel4 `seL4_UserContext` struct.
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

/// seL4 x86_64 syscall-register aliases used by shared TCB/IPC helpers.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum UserRegister {
    Rip,
    Rsp,
    Rflags,
    Rax,
    Rdi,
    Rsi,
    R10,
    R8,
    R9,
    R15,
    FsBase,
}

impl UserRegister {
    pub const fn index(self) -> usize {
        match self {
            Self::Rip => FAULT_IP,
            Self::Rsp => RSP,
            Self::Rflags => FLAGS,
            Self::Rax => RAX,
            Self::Rdi => RDI,
            Self::Rsi => RSI,
            Self::R10 => R10,
            Self::R8 => R8,
            Self::R9 => R9,
            Self::R15 => R15,
            Self::FsBase => FS_BASE,
        }
    }
}

/// Fault-reply register slots. Index 0 is the fault PC sentinel.
pub const UNKNOWN_SYSCALL_REPLY_REGS: [usize; 10] = [
    0,
    UserRegister::Rsp.index(),
    UserRegister::Rax.index(),
    UserRegister::Rdi.index(),
    UserRegister::Rsi.index(),
    UserRegister::R10.index(),
    UserRegister::R8.index(),
    UserRegister::R9.index(),
    UserRegister::R15.index(),
    5,
];
pub const USER_EXCEPTION_SP_REG: usize = UserRegister::Rsp.index();

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

pub fn same_object_as(left: Cap, right: Cap) -> bool {
    match (left.tag(), right.tag()) {
        (Some(CapTag::Endpoint), Some(CapTag::Endpoint)) => {
            left.endpoint_ptr() == right.endpoint_ptr()
        }
        (Some(CapTag::Notification), Some(CapTag::Notification)) => {
            left.notification_ptr() == right.notification_ptr()
        }
        (Some(CapTag::CNode), Some(CapTag::CNode)) => {
            left.cnode_ptr() == right.cnode_ptr() && left.cnode_radix() == right.cnode_radix()
        }
        (Some(CapTag::Thread), Some(CapTag::Thread)) => left.thread_ptr() == right.thread_ptr(),
        (Some(CapTag::Reply), Some(CapTag::Reply)) => {
            left.reply_object_ptr() == right.reply_object_ptr()
        }
        (Some(CapTag::IrqHandler), Some(CapTag::IrqHandler)) => {
            left.irq_handler_irq() == right.irq_handler_irq()
        }
        (Some(CapTag::Domain), Some(CapTag::Domain))
        | (Some(CapTag::AsidControl), Some(CapTag::AsidControl)) => true,
        (Some(CapTag::PageTable), Some(CapTag::PageTable)) => {
            left.page_table_base_ptr() == right.page_table_base_ptr()
        }
        (Some(CapTag::AsidPool), Some(CapTag::AsidPool)) => {
            left.asid_pool_ptr() == right.asid_pool_ptr()
        }
        (Some(CapTag::Frame), Some(CapTag::Frame)) => {
            left.frame_base_ptr() == right.frame_base_ptr()
                && left.frame_size() == right.frame_size()
                && left.frame_is_device() == right.frame_is_device()
        }
        _ => false,
    }
}
