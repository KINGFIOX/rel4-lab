//! RISC-V seL4 object-type IDs for Untyped_Retype.

use crate::abi::constants::{
    SEL4_ENDPOINT_BITS, SEL4_NOTIFICATION_BITS, SEL4_REPLY_BITS, SEL4_SLOT_BITS, SEL4_TCB_BITS,
};
use crate::object::cap::{
    Cap, FRAME_RIGHTS_READ_WRITE, FRAME_SIZE_4K, FRAME_SIZE_GIGAPAGE, FRAME_SIZE_MEGAPAGE,
};

#[repr(u64)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum ObjectType {
    Untyped = 0,
    Tcb = 1,
    Endpoint = 2,
    Notification = 3,
    CapTable = 4,
    GigaPage = 5,
    FourKPage = 6,
    MegaPage = 7,
    PageTable = 8,
    Reply = 9,
}

impl ObjectType {
    pub const fn from_raw(value: u64) -> Option<Self> {
        match value {
            0 => Some(Self::Untyped),
            1 => Some(Self::Tcb),
            2 => Some(Self::Endpoint),
            3 => Some(Self::Notification),
            4 => Some(Self::CapTable),
            5 => Some(Self::GigaPage),
            6 => Some(Self::FourKPage),
            7 => Some(Self::MegaPage),
            8 => Some(Self::PageTable),
            9 => Some(Self::Reply),
            _ => None,
        }
    }

    pub fn size_bits(self, user_size: u64) -> u64 {
        match self {
            Self::Untyped => user_size,
            Self::Tcb => SEL4_TCB_BITS as u64,
            Self::Endpoint => SEL4_ENDPOINT_BITS as u64,
            Self::Notification => SEL4_NOTIFICATION_BITS as u64,
            Self::CapTable => user_size + SEL4_SLOT_BITS as u64,
            Self::Reply => SEL4_REPLY_BITS as u64,
            Self::FourKPage | Self::PageTable => 12,
            Self::MegaPage => 21,
            Self::GigaPage => 30,
        }
    }

    pub fn device_retype_allowed(self) -> bool {
        matches!(
            self,
            Self::Untyped | Self::FourKPage | Self::MegaPage | Self::GigaPage
        )
    }

    pub fn create_cap(self, region_base: u64, user_size: u64, is_device: bool) -> Cap {
        match self {
            Self::Untyped => Cap::new_untyped(region_base, user_size, 0, is_device),
            Self::CapTable => Cap::new_cnode(region_base, user_size, 0, 0),
            Self::FourKPage => Cap::new_frame(
                region_base,
                FRAME_SIZE_4K,
                FRAME_RIGHTS_READ_WRITE,
                is_device,
            ),
            Self::MegaPage => Cap::new_frame(
                region_base,
                FRAME_SIZE_MEGAPAGE,
                FRAME_RIGHTS_READ_WRITE,
                is_device,
            ),
            Self::GigaPage => Cap::new_frame(
                region_base,
                FRAME_SIZE_GIGAPAGE,
                FRAME_RIGHTS_READ_WRITE,
                is_device,
            ),
            Self::PageTable => Cap::new_page_table(region_base),
            Self::Endpoint => Cap::new_endpoint(region_base),
            Self::Notification => Cap::new_notification(region_base),
            Self::Tcb => Cap::new_thread(region_base),
            Self::Reply => Cap::new_reply_object(region_base, true),
        }
    }
}
