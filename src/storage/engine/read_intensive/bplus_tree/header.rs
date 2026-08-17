use std::error::Error;

#[repr(u8)]
#[derive(Clone, Copy, PartialEq)]
pub(super) enum PageKind {
    Root = 0,
    Internal = 1,
    Leaf = 2,
}

impl PageKind {
    fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(PageKind::Root),
            1 => Some(PageKind::Internal),
            2 => Some(PageKind::Leaf),
            _ => None,
        }
    }
}

#[repr(C)]
pub(super) struct PageHeader {
    pub id: u64,           // 0-7
    pub free_start: u16,   // 8-9
    pub free_end: u16,     // 10-11
    pub page_ty: PageKind, // 12
    pub flags: u8,         // 13
    /// NOTE: ptr will behave as a Rightmost Pointers
    /// when the page is an Internal/Root node
    /// else as a right sibling leaf pointer when a Leaf node
    pub ptr: u64, // 14-21
                           // padding of 2
}

impl PageHeader {
    pub fn deserialize(buf: &[u8]) -> Option<Self> {
        Some(PageHeader {
            id: u64::from_le_bytes(buf[0..8].try_into().ok()?),
            free_start: u16::from_le_bytes(buf[8..10].try_into().ok()?),
            free_end: u16::from_le_bytes(buf[10..12].try_into().ok()?),
            page_ty: PageKind::from_u8(buf[12])?,
            flags: u8::from_le(buf[13]),
            ptr: u64::from_le_bytes(buf[14..22].try_into().ok()?),
        })
    }

    pub fn serialize(&self, buf: &mut [u8]) {
        buf[0..8].copy_from_slice(&self.id.to_le_bytes());
        buf[8..10].copy_from_slice(&self.free_start.to_le_bytes());
        buf[10..12].copy_from_slice(&self.free_end.to_le_bytes());
        buf[12] = self.page_ty as u8;
        buf[13] = self.flags;
        buf[14..22].copy_from_slice(&self.ptr.to_le_bytes());
    }
}
