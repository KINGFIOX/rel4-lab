#[derive(Copy, Clone)]
pub(crate) struct MappedTableLevel {
    pub range_bits: u32,
    pub object_type: u64,
    pub map_label: u64,
}

#[cfg(target_arch = "riscv64")]
pub(crate) mod riscv64;
#[cfg(target_arch = "riscv64")]
pub(crate) use riscv64 as current;

#[cfg(target_arch = "x86_64")]
pub(crate) mod x86_64;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64 as current;

#[cfg(not(any(target_arch = "riscv64", target_arch = "x86_64")))]
compile_error!("unsupported linux-compat target architecture");

#[cfg(target_arch = "riscv64")]
pub(crate) const PAGE_TABLE_LEVELS: &[MappedTableLevel] = &[
    MappedTableLevel {
        range_bits: 30,
        object_type: crate::consts::OBJ_PAGE_TABLE,
        map_label: crate::consts::LABEL_PAGE_TABLE_MAP,
    },
    MappedTableLevel {
        range_bits: 21,
        object_type: crate::consts::OBJ_PAGE_TABLE,
        map_label: crate::consts::LABEL_PAGE_TABLE_MAP,
    },
];

#[cfg(target_arch = "x86_64")]
pub(crate) const PAGE_TABLE_LEVELS: &[MappedTableLevel] = &[
    MappedTableLevel {
        range_bits: 39,
        object_type: crate::consts::OBJ_PDPT,
        map_label: crate::consts::LABEL_PDPT_MAP,
    },
    MappedTableLevel {
        range_bits: 30,
        object_type: crate::consts::OBJ_PAGE_DIRECTORY,
        map_label: crate::consts::LABEL_PAGE_DIRECTORY_MAP,
    },
    MappedTableLevel {
        range_bits: 21,
        object_type: crate::consts::OBJ_PAGE_TABLE,
        map_label: crate::consts::LABEL_PAGE_TABLE_MAP,
    },
];
