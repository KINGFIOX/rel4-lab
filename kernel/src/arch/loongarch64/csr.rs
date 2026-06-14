//! Thin helpers for LoongArch64 privileged CSR access.

use core::arch::asm;

pub const CSR_CRMD: usize = 0x000;
pub const CSR_PRMD: usize = 0x001;
pub const CSR_EUEN: usize = 0x002;
pub const CSR_ECFG: usize = 0x004;
pub const CSR_ESTAT: usize = 0x005;
pub const CSR_ERA: usize = 0x006;
pub const CSR_BADV: usize = 0x007;
pub const CSR_EENTRY: usize = 0x00c;
pub const CSR_TLBIDX: usize = 0x010;
pub const CSR_TLBEHI: usize = 0x011;
pub const CSR_TLBELO0: usize = 0x012;
pub const CSR_TLBELO1: usize = 0x013;
pub const CSR_ASID: usize = 0x018;
pub const CSR_PGDL: usize = 0x019;
pub const CSR_PGDH: usize = 0x01a;
pub const CSR_PGD: usize = 0x01b;
pub const CSR_PWCL: usize = 0x01c;
pub const CSR_PWCH: usize = 0x01d;
pub const CSR_STLBPS: usize = 0x01e;
pub const CSR_CPUID: usize = 0x020;
pub const CSR_KS0: usize = 0x030;
pub const CSR_TCFG: usize = 0x041;
pub const CSR_TVAL: usize = 0x042;
pub const CSR_TICLR: usize = 0x044;
pub const CSR_DMW0: usize = 0x180;
pub const CSR_DMW1: usize = 0x181;
pub const CSR_DMW2: usize = 0x182;
pub const CSR_DMW3: usize = 0x183;

const INVTLB_ALL: usize = 0x00;
const INVTLB_ASID: usize = 0x04;
const INVTLB_ADDR_G_OR_ASID: usize = 0x06;
pub const ASID_MASK: usize = 0x3ff;

macro_rules! ro_csr {
    ($name:ident, $csr:ident) => {
        #[inline]
        pub fn $name() -> usize {
            let value: usize;
            unsafe {
                asm!(
                    "csrrd {value}, {csr}",
                    value = out(reg) value,
                    csr = const $csr,
                    options(nostack, nomem)
                );
            }
            value
        }
    };
}

macro_rules! rw_csr {
    ($read:ident, $write:ident, $csr:ident) => {
        ro_csr!($read, $csr);

        #[inline]
        pub fn $write(value: usize) {
            unsafe {
                asm!(
                    "csrwr {value}, {csr}",
                    value = inlateout(reg) value => _,
                    csr = const $csr,
                    options(nostack, nomem)
                );
            }
        }
    };
}

rw_csr!(crmd, set_crmd, CSR_CRMD);
rw_csr!(prmd, set_prmd, CSR_PRMD);
rw_csr!(euen, set_euen, CSR_EUEN);
rw_csr!(ecfg, set_ecfg, CSR_ECFG);
rw_csr!(estat, set_estat, CSR_ESTAT);
rw_csr!(era, set_era, CSR_ERA);
ro_csr!(badv, CSR_BADV);
rw_csr!(eentry, set_eentry, CSR_EENTRY);
rw_csr!(tlbidx, set_tlbidx, CSR_TLBIDX);
rw_csr!(tlbehi, set_tlbehi, CSR_TLBEHI);
rw_csr!(tlbelo0, set_tlbelo0, CSR_TLBELO0);
rw_csr!(tlbelo1, set_tlbelo1, CSR_TLBELO1);
rw_csr!(asid, set_asid, CSR_ASID);
rw_csr!(pgdl, set_pgdl, CSR_PGDL);
rw_csr!(pgdh, set_pgdh, CSR_PGDH);
ro_csr!(pgd, CSR_PGD);
rw_csr!(pwcl, set_pwcl, CSR_PWCL);
rw_csr!(pwch, set_pwch, CSR_PWCH);
rw_csr!(stlbps, set_stlbps, CSR_STLBPS);
ro_csr!(cpuid, CSR_CPUID);
rw_csr!(ks0, set_ks0, CSR_KS0);
rw_csr!(tcfg, set_tcfg, CSR_TCFG);
ro_csr!(tval, CSR_TVAL);
rw_csr!(ticlr, set_ticlr, CSR_TICLR);
rw_csr!(dmw0, set_dmw0, CSR_DMW0);
rw_csr!(dmw1, set_dmw1, CSR_DMW1);
rw_csr!(dmw2, set_dmw2, CSR_DMW2);
rw_csr!(dmw3, set_dmw3, CSR_DMW3);

#[inline]
pub fn ibar() {
    unsafe { asm!("ibar 0", options(nostack, nomem)) };
}

#[inline]
pub fn dbar() {
    unsafe { asm!("dbar 0", options(nostack, nomem)) };
}

#[inline]
pub fn sscratch() -> usize {
    ks0()
}

#[inline]
pub fn set_sscratch(value: usize) {
    set_ks0(value);
}

#[inline]
pub fn iocsr_read32(addr: usize) -> u32 {
    let value: usize;
    unsafe {
        asm!(
            "iocsrrd.w {value}, {addr}",
            value = out(reg) value,
            addr = in(reg) addr,
            options(nostack)
        );
    }
    value as u32
}

#[inline]
pub fn iocsr_write32(addr: usize, value: u32) {
    unsafe {
        asm!(
            "iocsrwr.w {value}, {addr}",
            value = in(reg) value as usize,
            addr = in(reg) addr,
            options(nostack)
        );
    }
}

#[inline]
pub fn iocsr_read64(addr: usize) -> u64 {
    let value: usize;
    unsafe {
        asm!(
            "iocsrrd.d {value}, {addr}",
            value = out(reg) value,
            addr = in(reg) addr,
            options(nostack)
        );
    }
    value as u64
}

#[inline]
pub fn iocsr_write64(addr: usize, value: u64) {
    unsafe {
        asm!(
            "iocsrwr.d {value}, {addr}",
            value = in(reg) value as usize,
            addr = in(reg) addr,
            options(nostack)
        );
    }
}

#[inline]
pub fn sfence_vma_all() {
    unsafe {
        asm!("invtlb {op}, $zero, $zero", op = const INVTLB_ALL, options(nostack, nomem));
    }
    dbar();
}

#[inline]
pub fn sfence_vma_va(vaddr: usize) {
    let asid = asid() & ASID_MASK;
    unsafe {
        asm!(
            "invtlb {op}, {asid}, {vaddr}",
            op = const INVTLB_ADDR_G_OR_ASID,
            asid = in(reg) asid,
            vaddr = in(reg) vaddr,
            options(nostack, nomem)
        );
    }
    dbar();
}

#[inline]
pub fn sfence_vma_asid(asid: usize) {
    unsafe {
        asm!(
            "invtlb {op}, {asid}, $zero",
            op = const INVTLB_ASID,
            asid = in(reg) asid & ASID_MASK,
            options(nostack, nomem)
        );
    }
    dbar();
}

#[inline]
pub fn fence_i() {
    ibar();
}

#[inline]
pub fn time() -> usize {
    let value: usize;
    unsafe {
        asm!(
            "rdtime.d {value}, $zero",
            value = out(reg) value,
            options(nostack, nomem)
        );
    }
    value
}
