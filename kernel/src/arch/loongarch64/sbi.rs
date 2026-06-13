//! LoongArch64 placeholders for the RISC-V SBI call surface used by shared SMP code.

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct SbiRet {
    pub error: isize,
    pub value: usize,
}

const UNSUPPORTED: SbiRet = SbiRet {
    error: -1,
    value: 0,
};

pub fn send_ipi(_hart_mask: usize, _hart_mask_base: usize) -> SbiRet {
    UNSUPPORTED
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
