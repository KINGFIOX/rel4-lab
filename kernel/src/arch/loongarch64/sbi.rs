//! LoongArch64 IPI helpers behind the RISC-V SBI-shaped SMP call surface.

use crate::arch::loongarch64::csr;

pub const SUPPORTS_REMOTE_IPI: bool = true;
pub const SUPPORTS_REMOTE_TLB_FLUSH: bool = false;

const IOCSR_IPI_STATUS: usize = 0x1000;
const IOCSR_IPI_EN: usize = 0x1004;
const IOCSR_IPI_CLEAR: usize = 0x100c;
const IOCSR_IPI_SEND: usize = 0x1040;
const IPI_SEND_ACTION_RESCHEDULE: u64 = 1;
const IPI_SEND_CPU_SHIFT: usize = 16;
const IPI_SEND_BLOCKING: u64 = 1 << 31;

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct SbiRet {
    pub error: isize,
    pub value: usize,
}

const UNSUPPORTED: SbiRet = SbiRet {
    error: -1,
    value: 0,
};

const OK: SbiRet = SbiRet { error: 0, value: 0 };

#[inline]
pub fn init_ipi() {
    csr::iocsr_write64(IOCSR_IPI_EN, u64::MAX);
}

#[inline]
pub fn ack_ipi() -> bool {
    let pending = csr::iocsr_read64(IOCSR_IPI_STATUS);
    if pending == 0 {
        return false;
    }
    csr::iocsr_write64(IOCSR_IPI_CLEAR, pending);
    true
}

pub fn send_ipi(hart_mask: usize, hart_mask_base: usize) -> SbiRet {
    let mut mask = hart_mask;
    let mut bit = 0usize;
    while mask != 0 {
        if mask & 1 != 0 {
            let cpu = hart_mask_base + bit;
            csr::iocsr_write64(
                IOCSR_IPI_SEND,
                IPI_SEND_BLOCKING
                    | ((cpu as u64) << IPI_SEND_CPU_SHIFT)
                    | IPI_SEND_ACTION_RESCHEDULE,
            );
        }
        mask >>= 1;
        bit += 1;
    }
    OK
}

pub fn remote_fence_i(_hart_mask: usize, _hart_mask_base: usize) -> SbiRet {
    UNSUPPORTED
}

pub fn remote_sfence_vma(
    _hart_mask: usize,
    _hart_mask_base: usize,
    _start: usize,
    _size: usize,
) -> SbiRet {
    UNSUPPORTED
}

pub fn remote_sfence_vma_asid(
    _hart_mask: usize,
    _hart_mask_base: usize,
    _start: usize,
    _size: usize,
    _asid: usize,
) -> SbiRet {
    UNSUPPORTED
}

pub fn shutdown() -> ! {
    crate::arch::current::boot::halt()
}
