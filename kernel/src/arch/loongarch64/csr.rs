//! Thin helpers for LoongArch64 privileged CSR access.

use core::arch::asm;

pub const CSR_CRMD: usize = 0x000;
pub const CSR_PRMD: usize = 0x001;
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
pub const CSR_CPUID: usize = 0x020;
pub const CSR_TCFG: usize = 0x041;
pub const CSR_TVAL: usize = 0x042;
pub const CSR_TICLR: usize = 0x044;
pub const CSR_DMW0: usize = 0x180;
pub const CSR_DMW1: usize = 0x181;
pub const CSR_DMW2: usize = 0x182;
pub const CSR_DMW3: usize = 0x183;

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
ro_csr!(cpuid, CSR_CPUID);
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
