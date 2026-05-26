//! Minimal RISC-V SBI (Supervisor Binary Interface) call wrappers.
//!
//! We use the legacy extensions only — they are universally supported by the
//! OpenSBI implementation that ships inside the seL4 elfloader bundle:
//!
//! - `0x00` — `sbi_set_timer(stime_value: u64)`
//! - `0x01` — `sbi_console_putchar(ch: i32)`
//! - `0x02` — `sbi_console_getchar() -> i32`
//! - `0x08` — `sbi_shutdown()`
//!
//! The legacy ABI passes the function ID in `a7` and returns a single value
//! in `a0` (errors via negative values).

use core::arch::asm;

#[inline]
unsafe fn ecall1(eid: usize, arg0: usize) -> usize {
    let ret: usize;
    unsafe {
        asm!(
            "ecall",
            in("a7") eid,
            inlateout("a0") arg0 => ret,
            options(nostack),
        );
    }
    ret
}

#[inline]
pub fn console_putchar(ch: u8) {
    let _ = unsafe { ecall1(0x01, ch as usize) };
}

#[inline]
#[allow(dead_code)]
pub fn console_getchar() -> i32 {
    unsafe { ecall1(0x02, 0) as i32 }
}

#[inline]
#[allow(dead_code)]
pub fn set_timer(stime_value: u64) {
    let _ = unsafe { ecall1(0x00, stime_value as usize) };
}

#[inline]
#[allow(dead_code)]
pub fn shutdown() -> ! {
    unsafe {
        let _ = ecall1(0x08, 0);
    }
    loop {
        unsafe { asm!("wfi") };
    }
}
