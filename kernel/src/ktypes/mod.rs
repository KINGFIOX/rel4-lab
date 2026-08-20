//! Safe primitives the rest of the kernel is built on.
//!
//! Everything in this module exists so that the kernel's object layer, API
//! layer, and device drivers can be written without `unsafe`. Raw-pointer
//! dereferences, volatile device access, and `static mut`-style global state
//! are confined here, behind types that encode the invariants which used to
//! live only in prose:
//!
//! * [`objref`] wraps the KVAs the seL4 object model stores inside caps and
//!   inside objects, and turns "dereference under the big kernel lock" into a
//!   scoped borrow.
//! * [`list`] owns the intrusive queue links; it is the only code allowed to
//!   write a link field, so the "a TCB is on at most one queue" invariant has
//!   a single place to be argued.
//! * [`addr`] separates physical addresses from the kernel window and from
//!   user virtual addresses.
//! * [`mmio`] wraps volatile device registers.
//! * [`percpu`] and [`once`] replace `static mut` and ad-hoc `UnsafeCell`
//!   wrappers for boot-time-initialised globals.

pub mod addr;
pub mod list;
pub mod mmio;
pub mod objref;
pub mod once;
pub mod percpu;
