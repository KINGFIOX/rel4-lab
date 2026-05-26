//! Kernel object types and capability bookkeeping.
//!
//! Every kernel object the user can refer to (untyped memory, CNode, TCB,
//! endpoint, frame, page table, …) gets a `cap_t` representation; the
//! mapping `cptr → cap_t + cte_t` lives in a CNode. The capability
//! derivation tree (CDT) is maintained as a doubly-linked list of `cte_t`
//! nodes via the embedded `mdb_node_t`.
//!
//! Bit layouts mirror the C kernel's generated headers exactly. See
//! `kernel/generated/arch/object/structures_gen.h` for the source of
//! truth.

pub mod cap;
pub mod cnode;
pub mod mdb;
pub mod untyped;
