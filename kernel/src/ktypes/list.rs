//! Intrusive FIFO queues over kernel objects.
//!
//! Runqueues and the endpoint/notification wait lists are all doubly-linked
//! lists whose links live inside the queued TCB. The links are private to this
//! module: a node type embeds [`Links`] and implements [`Linked`] to project
//! to it, and only the operations here read or write `prev`/`next`. That gives
//! the "a TCB is on at most one queue at a time" invariant a single place to be
//! maintained, instead of being re-argued at every enqueue and dequeue site.
//!
//! Where the head and tail pointers live differs per queue: a runqueue keeps
//! them in a plain [`Queue`], while endpoints and notifications pack them into
//! the object words their ABI fixes. Both cases are served by the
//! [`QueueEnds`] trait, so the linking logic is written once.

use crate::ktypes::objref::ObjRef;

/// Queue links embedded in a queueable object.
///
/// Layout is two words, matching the pair of KVAs the object ABI reserves for
/// them, and the fields are private so that only this module can relink a
/// node.
#[repr(C)]
pub struct Links<T> {
    prev: Option<ObjRef<T>>,
    next: Option<ObjRef<T>>,
}

// Derives would demand `T: Copy` and friends on the queued object itself,
// which is a whole kernel object; the links only ever hold handles.
impl<T> Clone for Links<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for Links<T> {}

impl<T> PartialEq for Links<T> {
    fn eq(&self, other: &Self) -> bool {
        self.prev == other.prev && self.next == other.next
    }
}

impl<T> Eq for Links<T> {}

impl<T> core::fmt::Debug for Links<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Links")
            .field("prev", &self.prev)
            .field("next", &self.next)
            .finish()
    }
}

impl<T> Links<T> {
    pub const fn unlinked() -> Self {
        Self {
            prev: None,
            next: None,
        }
    }

    /// True when the node is not on any queue.
    #[inline]
    pub fn is_unlinked(&self) -> bool {
        self.prev.is_none() && self.next.is_none()
    }
}

impl<T> Default for Links<T> {
    fn default() -> Self {
        Self::unlinked()
    }
}

/// An object that can be queued, by exposing the [`Links`] it embeds.
///
/// # Safety
/// `links` must return the same field every time, and that field must not be
/// aliased by any other accessor, so that queue operations are the only writer
/// of the node's link state.
pub unsafe trait Linked: Sized {
    fn links(this: &mut Self) -> &mut Links<Self>;
}

/// Where a queue keeps its head and tail.
///
/// Implementors range from a plain struct of two handles to an endpoint object
/// that packs the head pointer together with its state bits.
pub trait QueueEnds<T: Linked> {
    fn head(&self) -> Option<ObjRef<T>>;
    fn tail(&self) -> Option<ObjRef<T>>;
    fn set_head(&mut self, head: Option<ObjRef<T>>);
    fn set_tail(&mut self, tail: Option<ObjRef<T>>);
}

/// A FIFO queue that owns its head and tail pointers.
pub struct Queue<T> {
    head: Option<ObjRef<T>>,
    tail: Option<ObjRef<T>>,
}

impl<T> Clone for Queue<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for Queue<T> {}

impl<T> Queue<T> {
    pub const fn new() -> Self {
        Self {
            head: None,
            tail: None,
        }
    }
}

impl<T> Default for Queue<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Linked> QueueEnds<T> for Queue<T> {
    #[inline]
    fn head(&self) -> Option<ObjRef<T>> {
        self.head
    }

    #[inline]
    fn tail(&self) -> Option<ObjRef<T>> {
        self.tail
    }

    #[inline]
    fn set_head(&mut self, head: Option<ObjRef<T>>) {
        self.head = head;
    }

    #[inline]
    fn set_tail(&mut self, tail: Option<ObjRef<T>>) {
        self.tail = tail;
    }
}

#[inline]
fn links_of<T: Linked>(node: ObjRef<T>) -> Links<T> {
    node.with_mut(|n| *T::links(n))
}

#[inline]
fn set_links<T: Linked>(node: ObjRef<T>, links: Links<T>) {
    node.with_mut(|n| *T::links(n) = links);
}

#[inline]
fn set_next<T: Linked>(node: ObjRef<T>, next: Option<ObjRef<T>>) {
    node.with_mut(|n| T::links(n).next = next);
}

#[inline]
fn set_prev<T: Linked>(node: ObjRef<T>, prev: Option<ObjRef<T>>) {
    node.with_mut(|n| T::links(n).prev = prev);
}

/// True when `node` is on no queue.
#[inline]
pub fn is_unlinked<T: Linked>(node: ObjRef<T>) -> bool {
    links_of(node).is_unlinked()
}

/// Forget `node`'s links without touching any queue.
///
/// Only correct for a node that has already been unlinked from its queue, or
/// whose queue is being torn down wholesale.
#[inline]
pub fn clear_links<T: Linked>(node: ObjRef<T>) {
    set_links(node, Links::unlinked());
}

/// Successor of `node` in whichever queue it is on.
#[inline]
pub fn next_of<T: Linked>(node: ObjRef<T>) -> Option<ObjRef<T>> {
    links_of(node).next
}

/// Predecessor of `node` in whichever queue it is on.
#[inline]
pub fn prev_of<T: Linked>(node: ObjRef<T>) -> Option<ObjRef<T>> {
    links_of(node).prev
}

/// Append `node` to the tail of the queue.
pub fn push_back<T: Linked>(ends: &mut impl QueueEnds<T>, node: ObjRef<T>) {
    let tail = ends.tail();
    debug_assert!(
        tail != Some(node),
        "queue: node is already this queue's tail"
    );
    set_links(
        node,
        Links {
            prev: tail,
            next: None,
        },
    );
    match tail {
        Some(tail) => set_next(tail, Some(node)),
        None => ends.set_head(Some(node)),
    }
    ends.set_tail(Some(node));
}

/// Remove and return the queue's first node.
pub fn pop_front<T: Linked>(ends: &mut impl QueueEnds<T>) -> Option<ObjRef<T>> {
    let head = ends.head()?;
    let next = links_of(head).next;
    match next {
        Some(next) => set_prev(next, None),
        None => ends.set_tail(None),
    }
    ends.set_head(next);
    clear_links(head);
    Some(head)
}

/// Unlink `node` from the queue it is assumed to be on.
pub fn remove<T: Linked>(ends: &mut impl QueueEnds<T>, node: ObjRef<T>) {
    let Links { prev, next } = links_of(node);
    match prev {
        Some(prev) => set_next(prev, next),
        None => ends.set_head(next),
    }
    match next {
        Some(next) => set_prev(next, prev),
        None => ends.set_tail(prev),
    }
    clear_links(node);
}

/// Unlink `node` only if it really is on this queue, reporting whether it was.
pub fn remove_if_present<T: Linked>(ends: &mut impl QueueEnds<T>, node: ObjRef<T>) -> bool {
    if !contains(ends, node) {
        return false;
    }
    remove(ends, node);
    true
}

/// Whether `node` is reachable from the queue's head.
pub fn contains<T: Linked>(ends: &impl QueueEnds<T>, node: ObjRef<T>) -> bool {
    iter(ends).any(|entry| entry == node)
}

/// Walk the queue from head to tail.
///
/// The queue must not be modified while iterating; call sites that unlink
/// nodes collect them first, or restart the walk.
pub fn iter<T: Linked>(ends: &impl QueueEnds<T>) -> Iter<T> {
    Iter { at: ends.head() }
}

pub struct Iter<T> {
    at: Option<ObjRef<T>>,
}

impl<T: Linked> Iterator for Iter<T> {
    type Item = ObjRef<T>;

    fn next(&mut self) -> Option<Self::Item> {
        let current = self.at?;
        self.at = links_of(current).next;
        Some(current)
    }
}

const _: () = {
    // Object ABIs reserve exactly two words for the links.
    assert!(size_of::<Links<u8>>() == 2 * size_of::<u64>());
    assert!(align_of::<Links<u8>>() == align_of::<u64>());
};
