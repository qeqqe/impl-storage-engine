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
    pub id: u64,           // 0..8
    pub free_start: u16,   // 8..10
    pub free_end: u16,     // 10..12
    pub page_ty: PageKind, // 12
    pub flags: u8,         // 13, flag_bits[0] == primary_page, flage_bits[1] == has_overflow_page
    /// NOTE: ptr will behave as a Rightmost Pointers
    /// when the page is an Internal/Root node
    /// else as a right sibling leaf pointer when a Leaf node...
    /// ___
    /// More importantly, ptr will also behave differently depending
    /// on the flags.
    /// As a convention, if the first bit of the flag is 1,
    /// that would mean that THIS page is a primary page, else it's an overflow page.
    ///
    /// If the second bit of the flag is 1 that would mean that this page HAS an overflow page.
    ///
    /// So depending on the flags we have this combination (FB = flag bit)
    ///
    /// FB\[0\] == true && FB\[1\] == false: A primary page, where `ptr` indicates the child/sibling
    /// page id depending on the PageKind.
    ///
    /// FB\[0\] == true && FB\[1\] == true: A primary page that has an overflow page, where `ptr`
    /// indicated the page id of the overflow'd page.
    ///
    /// FB\[0\] == false && FB\[1\] == true: An overflow page that also has an overflow page, where `ptr`
    /// indicated the page id of the overflow'd page.
    ///
    /// FB\[0\] == false && FB\[1\] == false: An overflow page that doesn't have an overflow page, where `ptr`indicates
    /// page id of the child/sibling depending on the PageKind.
    ///
    /// This way we can make page header not have extra data for the overflow pages.
    pub ptr: u64, // 14..22
} // 22

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

///     |----------------|
///     | file header    |   
///     |----------------|
///     | page header    |   
///     |----------------|
///     | cell pointer   |   |  4 bytes per cell pointer.  Sorted order.
///     | array          |   |  Grows downward
///     |                |   v
///     |----------------|
///     | unallocated    |
///     | space          |
///     |----------------|   ^  Grows upwards
///     | cell content   |   |  Arbitrary order interspersed with freeblocks.
///     | area           |   |  and free space fragments.
///     |----------------|
///
///     (referenced from sqlite)
pub(super) struct HeapHeader {
    pub id: u64,         // 0..8
    pub free_start: u16, // 8..10
    pub free_end: u16,   // 10..12
    pub flags: u8,       // 12
} // 13

impl HeapHeader {
    pub fn deserialize(buf: &[u8]) -> Option<Self> {
        Some(HeapHeader {
            id: u64::from_le_bytes(buf[0..8].try_into().ok()?),
            free_start: u16::from_le_bytes(buf[8..10].try_into().ok()?),
            free_end: u16::from_le_bytes(buf[10..12].try_into().ok()?),
            flags: u8::from_le(buf[12]),
        })
    }

    pub fn serialize(&self, buf: &mut [u8]) {
        buf[0..8].copy_from_slice(&self.id.to_le_bytes());
        buf[8..10].copy_from_slice(&self.free_start.to_le_bytes());
        buf[10..12].copy_from_slice(&self.free_end.to_le_bytes());
        buf[12] = self.flags;
    }
}
