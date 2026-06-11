#[derive(Copy, Clone, PartialEq, Eq)]
pub enum DiskOp {
    Read,
    Write,
    Flush,
}

impl DiskOp {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Flush => "flush",
        }
    }
}

#[derive(Copy, Clone)]
pub struct InFlightRequest {
    pub active: bool,
    pub completed: bool,
    pub op: DiskOp,
    pub blockno: u64,
    pub shared_slot: u64,
    pub reply: [u64; 4],
}

impl InFlightRequest {
    pub const fn none() -> Self {
        Self {
            active: false,
            completed: false,
            op: DiskOp::Read,
            blockno: 0,
            shared_slot: 0,
            reply: [0; 4],
        }
    }
}

#[derive(Copy, Clone)]
pub struct ReplyTarget {
    pub async_completion: bool,
    pub completion_id: u64,
}

impl ReplyTarget {
    pub const fn caller() -> Self {
        Self {
            async_completion: false,
            completion_id: 0,
        }
    }

    pub const fn completion(completion_id: u64) -> Self {
        Self {
            async_completion: true,
            completion_id,
        }
    }
}

pub enum RequestResult {
    Reply([u64; 4]),
    Deferred,
}
