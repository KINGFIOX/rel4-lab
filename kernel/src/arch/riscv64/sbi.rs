//! Minimal RISC-V SBI (Supervisor Binary Interface) call wrappers.
//!
//! SBI v0.2+ calls pass the extension ID in `a7`, the function ID in `a6`,
//! arguments in `a0..a5`, and return `sbiret { error: a0, value: a1 }`.
//!
//! OpenSBI v0.9 does not expose the Debug Console extension, and we avoid
//! the legacy console extension entirely. Console I/O is handled by the
//! machine UART layer instead.

use core::arch::asm;

pub const SUPPORTS_REMOTE_IPI: bool = true;
pub const SUPPORTS_REMOTE_TLB_FLUSH: bool = true;

#[repr(usize)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum SbiExtension {
    Ipi = 0x7350_49,
    Rfence = 0x5246_4e43,
    Time = 0x5449_4d45,
    SystemReset = 0x5352_5354,
}

impl SbiExtension {
    const fn raw(self) -> usize {
        self as usize
    }
}

#[repr(usize)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum TimeFunction {
    SetTimer = 0,
}

impl TimeFunction {
    const fn raw(self) -> usize {
        self as usize
    }
}

#[repr(usize)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum IpiFunction {
    SendIpi = 0,
}

impl IpiFunction {
    const fn raw(self) -> usize {
        self as usize
    }
}

#[repr(usize)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum RfenceFunction {
    RemoteFenceI = 0,
    RemoteSfenceVma = 1,
    RemoteSfenceVmaAsid = 2,
}

impl RfenceFunction {
    const fn raw(self) -> usize {
        self as usize
    }
}

#[repr(usize)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum SystemResetFunction {
    Reset = 0,
}

impl SystemResetFunction {
    const fn raw(self) -> usize {
        self as usize
    }
}

#[repr(usize)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum ResetType {
    Shutdown = 0,
}

impl ResetType {
    const fn raw(self) -> usize {
        self as usize
    }
}

#[repr(usize)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum ResetReason {
    None = 0,
}

impl ResetReason {
    const fn raw(self) -> usize {
        self as usize
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct SbiRet {
    pub error: isize,
    pub value: usize,
}

#[inline]
unsafe fn ecall6(eid: usize, fid: usize, args: [usize; 6]) -> SbiRet {
    let error_word: usize;
    let value: usize;
    unsafe {
        asm!(
            "ecall",
            in("a7") eid,
            in("a6") fid,
            inlateout("a0") args[0] => error_word,
            inlateout("a1") args[1] => value,
            in("a2") args[2],
            in("a3") args[3],
            in("a4") args[4],
            in("a5") args[5],
            options(nostack),
        );
    }
    SbiRet {
        error: error_word as isize,
        value,
    }
}

#[inline]
fn call(eid: usize, fid: usize, arg0: usize, arg1: usize, arg2: usize) -> SbiRet {
    unsafe { ecall6(eid, fid, [arg0, arg1, arg2, 0, 0, 0]) }
}

#[inline]
fn call6(eid: usize, fid: usize, args: [usize; 6]) -> SbiRet {
    unsafe { ecall6(eid, fid, args) }
}

#[inline]
fn wfi_forever() -> ! {
    loop {
        unsafe {
            asm!("wfi", options(nomem, nostack));
        }
    }
}

#[inline]
#[allow(dead_code)]
pub fn set_timer(stime_value: u64) {
    let _ = call(
        SbiExtension::Time.raw(),
        TimeFunction::SetTimer.raw(),
        stime_value as usize,
        0,
        0,
    );
}

#[inline]
#[allow(dead_code)]
pub fn send_ipi(hart_mask: usize, hart_mask_base: usize) -> SbiRet {
    call(
        SbiExtension::Ipi.raw(),
        IpiFunction::SendIpi.raw(),
        hart_mask,
        hart_mask_base,
        0,
    )
}

#[inline]
#[allow(dead_code)]
pub fn remote_fence_i(hart_mask: usize, hart_mask_base: usize) -> SbiRet {
    call(
        SbiExtension::Rfence.raw(),
        RfenceFunction::RemoteFenceI.raw(),
        hart_mask,
        hart_mask_base,
        0,
    )
}

#[inline]
#[allow(dead_code)]
pub fn remote_sfence_vma(
    hart_mask: usize,
    hart_mask_base: usize,
    start: usize,
    size: usize,
) -> SbiRet {
    call6(
        SbiExtension::Rfence.raw(),
        RfenceFunction::RemoteSfenceVma.raw(),
        [hart_mask, hart_mask_base, start, size, 0, 0],
    )
}

#[inline]
#[allow(dead_code)]
pub fn remote_sfence_vma_asid(
    hart_mask: usize,
    hart_mask_base: usize,
    start: usize,
    size: usize,
    asid: usize,
) -> SbiRet {
    call6(
        SbiExtension::Rfence.raw(),
        RfenceFunction::RemoteSfenceVmaAsid.raw(),
        [hart_mask, hart_mask_base, start, size, asid, 0],
    )
}

#[inline]
#[allow(dead_code)]
pub fn shutdown() -> ! {
    let _ = call(
        SbiExtension::SystemReset.raw(),
        SystemResetFunction::Reset.raw(),
        ResetType::Shutdown.raw(),
        ResetReason::None.raw(),
        0,
    );
    wfi_forever()
}
