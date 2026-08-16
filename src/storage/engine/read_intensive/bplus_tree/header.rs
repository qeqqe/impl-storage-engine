#[repr(u8)]
#[derive(Clone, Copy, PartialEq)]
enum PageKind {
    Root = 0,
    Internal = 1,
    Leaf = 2,
}

#[repr(C)]
struct PageHeader {
    id: u64,           // 0-7
    free_start: u16,   // 8-9
    free_end: u16,     // 10-11
    page_ty: PageKind, // 12
    flags: u8,         // 13
                       // 2 bytes tail padding, unavoidable cus u64 forces align-8
}

#[repr(C)]
struct LeafHeader {
    common: PageHeader, // 16
    right_ptr: u64,     // 8, right most leaf node = 0
}

#[repr(C)]
struct InternalHeader {
    common: PageHeader,   // 16
    rightmost_child: u64, // 8, last child pointer
}
