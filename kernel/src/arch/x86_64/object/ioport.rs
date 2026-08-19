//! Issued x86 I/O port ranges. Mirrors seL4 `x86KSAllocatedIOPorts`.

use crate::kernel::smp::BklCell;

const PORT_WORDS: usize = 65536 / 64;

static ALLOCATED: BklCell<[u64; PORT_WORDS]> = BklCell::new([0; PORT_WORDS]);

fn word_bit(port: u16) -> (usize, u64) {
    (usize::from(port) / 64, 1u64 << (port % 64))
}

pub fn range_free(first: u16, last: u16) -> bool {
    ALLOCATED.with_ref(|bits| {
        let mut port = first;
        loop {
            let (word, mask) = word_bit(port);
            if bits[word] & mask != 0 {
                return false;
            }
            if port == last {
                return true;
            }
            port = port.wrapping_add(1);
        }
    })
}

pub fn alloc_range(first: u16, last: u16) {
    ALLOCATED.with_mut(|bits| set_range(bits, first, last, true));
}

pub fn free_range(first: u16, last: u16) {
    ALLOCATED.with_mut(|bits| set_range(bits, first, last, false));
}

fn set_range(bits: &mut [u64; PORT_WORDS], first: u16, last: u16, allocated: bool) {
    let mut port = first;
    loop {
        let (word, mask) = word_bit(port);
        if allocated {
            bits[word] |= mask;
        } else {
            bits[word] &= !mask;
        }
        if port == last {
            return;
        }
        port = port.wrapping_add(1);
    }
}
