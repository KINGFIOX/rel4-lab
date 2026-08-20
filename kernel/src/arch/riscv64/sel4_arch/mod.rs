//! RISC-V 64-bit seL4 ABI surface: user context accessors, invocation
//! labels, and object-type IDs. Shared kernel code talks to this module
//! instead of RISC-V register or SATP names.

pub mod invocation;
pub mod object_type;

pub use crate::arch::riscv64::kernel::trap::{
    ROOTSERVER_SSTATUS, SEL4_TCB_FRAME_REGS, SEL4_TCB_GP_REGS, SEL4_USER_CONTEXT_ABI_WORDS,
    SEL4_USER_CONTEXT_REGS, SEL4_USER_CONTEXT_WORDS, SSTATUS_FS_CLEAN, SSTATUS_FS_MASK,
    SSTATUS_SPIE, SSTATUS_SPP, USER_SSTATUS, UserContext, UserRegister,
};
pub use object_type::ObjectType;

/// Opaque address-space root programmed by `switch_vspace`.
pub type VspaceRoot = u64;

use crate::arch::riscv64::kernel::trap::UserRegister as Reg;

impl UserContext {
    pub fn cap_reg(&self) -> u64 {
        self.regs[Reg::A0.index()]
    }

    pub fn set_cap_reg(&mut self, value: u64) {
        self.regs[Reg::A0.index()] = value;
    }

    pub fn msg_info(&self) -> u64 {
        self.regs[Reg::A1.index()]
    }

    pub fn set_msg_info(&mut self, value: u64) {
        self.regs[Reg::A1.index()] = value;
    }

    pub fn mr(&self, index: usize) -> u64 {
        match index {
            0 => self.regs[Reg::A2.index()],
            1 => self.regs[Reg::A3.index()],
            2 => self.regs[Reg::A4.index()],
            3 => self.regs[Reg::A5.index()],
            _ => 0,
        }
    }

    pub fn set_mr(&mut self, index: usize, value: u64) {
        let slot = match index {
            0 => Reg::A2.index(),
            1 => Reg::A3.index(),
            2 => Reg::A4.index(),
            3 => Reg::A5.index(),
            _ => return,
        };
        self.regs[slot] = value;
    }

    pub fn reply_reg(&self) -> u64 {
        self.regs[Reg::A6.index()]
    }

    pub fn set_reply_reg(&mut self, value: u64) {
        self.regs[Reg::A6.index()] = value;
    }

    pub fn syscall_reg(&self) -> u64 {
        self.regs[Reg::A7.index()]
    }

    pub fn stack_reg(&self) -> u64 {
        self.regs[Reg::Sp.index()]
    }

    pub fn set_stack_reg(&mut self, value: u64) {
        self.regs[Reg::Sp.index()] = value;
    }

    pub fn return_reg(&self) -> u64 {
        self.regs[Reg::Ra.index()]
    }

    pub fn set_return_reg(&mut self, value: u64) {
        self.regs[Reg::Ra.index()] = value;
    }

    pub fn tls_reg(&self) -> u64 {
        self.regs[Reg::Tp.index()]
    }

    pub fn set_tls_reg(&mut self, value: u64) {
        self.regs[Reg::Tp.index()] = value;
    }

    pub fn scratch_reg(&self) -> u64 {
        self.regs[Reg::T0.index()]
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

/// Fault-reply register slots. MR 0 is FaultIP.
pub const UNKNOWN_SYSCALL_LENGTH: u64 = 11;
pub const UNKNOWN_SYSCALL_FAULT_IP_MR: usize = 0;
pub const UNKNOWN_SYSCALL_REPLY_REGS: &[usize] = &[
    0,
    UserRegister::Sp.index(),
    UserRegister::Ra.index(),
    UserRegister::A0.index(),
    UserRegister::A1.index(),
    UserRegister::A2.index(),
    UserRegister::A3.index(),
    UserRegister::A4.index(),
    UserRegister::A5.index(),
    UserRegister::A6.index(),
    0,
];
pub const USER_EXCEPTION_SP_REG: usize = UserRegister::Sp.index();

pub fn init_user_context(context: &mut UserContext) {
    context.sstatus = USER_SSTATUS;
}

pub fn init_rootserver_context(context: &mut UserContext, entry: u64, stack: u64, bootinfo: u64) {
    context.pc = entry;
    context.restart_pc = entry;
    context.sstatus = ROOTSERVER_SSTATUS;
    context.set_cap_reg(bootinfo);
    context.set_msg_info(0);
    context.set_stack_reg(stack);
}

/// Fill an idle TCB as seL4 `Arch_configureIdleThread` does: NextIP
/// is `idle_thread`, S-mode with interrupts enabled on `sret`, SP is the
/// kernel stack. The trap path does not restore this context today.
pub fn configure_idle_context(context: &mut UserContext, kernel_sp: u64) {
    let pc = idle_thread as *const () as usize as u64;
    context.pc = pc;
    context.restart_pc = pc;
    context.sstatus = SSTATUS_SPP | SSTATUS_SPIE;
    context.set_stack_reg(kernel_sp);
}

/// seL4 RISC-V `idle_thread`: wait for an interrupt. Stored as the idle
/// TCB program counter; the current kernel waits in `idle_scheduler_loop`
/// instead of `sret`ing here.
#[unsafe(no_mangle)]
pub extern "C" fn idle_thread() -> ! {
    loop {
        // SAFETY: waiting until the next interrupt.
        unsafe {
            core::arch::asm!("wfi", options(nomem, nostack));
        }
    }
}

pub fn apply_written_pc(context: &mut UserContext, pc: u64) {
    context.pc = pc;
    context.restart_pc = pc;
}

pub fn apply_preemption_restart(context: &mut UserContext) {
    apply_written_pc(context, context.restart_pc);
}

pub fn set_fpu_context_enabled(context: &mut UserContext, enabled: bool) {
    let status = context.sstatus & !SSTATUS_FS_MASK;
    context.sstatus = if enabled {
        status | SSTATUS_FS_CLEAN
    } else {
        status
    };
}
