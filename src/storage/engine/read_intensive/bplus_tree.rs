pub(super) mod header;

const PAGE_SIZE: usize = 4096;
const HEADER_SIZE: usize = 8 + 2 + 2 + 1 + 1 + 8;
const KEY_SIZE: usize = 8;
const PTR_SIZE: usize = 8;

// max seperator keys in a page
const ORDER: usize = (PAGE_SIZE - HEADER_SIZE) / (KEY_SIZE + PTR_SIZE);
const FANOUT: usize = ORDER + 1;

struct BplusTree {
    root_id: u64,
}

impl BplusTree {}
