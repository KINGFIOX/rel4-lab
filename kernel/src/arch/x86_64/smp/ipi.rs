pub const SUPPORTS_REMOTE_IPI: bool = false;
pub const SUPPORTS_REMOTE_TLB_FLUSH: bool = false;

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct IpiRet {
    pub error: isize,
    pub value: usize,
}

const UNSUPPORTED: IpiRet = IpiRet {
    error: -1,
    value: 0,
};

pub fn send_ipi(_mask: usize, _hart_id: usize) -> IpiRet {
    UNSUPPORTED
}

pub fn remote_sfence_vma(_mask: usize, _hart_id: usize, _start: usize, _size: usize) -> IpiRet {
    UNSUPPORTED
}

pub fn remote_sfence_vma_asid(
    _mask: usize,
    _hart_id: usize,
    _start: usize,
    _size: usize,
    _asid: usize,
) -> IpiRet {
    UNSUPPORTED
}

pub fn ack_ipi() {}
