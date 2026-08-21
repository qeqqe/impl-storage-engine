use std::{error::Error, io::ErrorKind::NetworkDown};

use crate::storage::engine::read_intensive::bplus_tree::header::HeapHeader;

use super::{
    HEADER_SIZE, KEY_SIZE, PAGE_SIZE, PTR_SIZE, SLOT_SIZE,
    header::{PageHeader, PageKind},
};

pub(super) struct Page {
    pub data: [u8; PAGE_SIZE],
}

impl Page {
    pub fn get_cells(&self) -> Result<Vec<Cell>, Box<dyn Error>> {
        let p_hdr = self.header()?;
        let range = ((p_hdr.free_start as u32 - HEADER_SIZE as u32) / 4) as u16; // 4 bytes per ptr

        let mut cells = Vec::with_capacity(range as usize);
        match p_hdr.page_ty {
            PageKind::Leaf => {
                for i in 0..range {
                    let slot = self.slot(i);
                    let start = slot.cell_offset as usize;
                    let key = u64::from_le_bytes(self.data[start..start + 8].try_into().unwrap());
                    let index =
                        u64::from_le_bytes(self.data[start + 8..start + 16].try_into().unwrap());
                    cells.push(Cell {
                        key,
                        c_ptr: None,
                        h_ptr: Some(HeapPointer { index }),
                    });
                }
            }
            _ => {
                for i in 0..range {
                    let slot = self.slot(i);
                    let start = slot.cell_offset as usize;
                    let key = u64::from_le_bytes(self.data[start..start + 8].try_into().unwrap());
                    let c_ptr =
                        u64::from_le_bytes(self.data[start + 8..start + 12].try_into().unwrap());
                    cells.push(Cell {
                        key,
                        c_ptr: Some(c_ptr),
                        h_ptr: None,
                    });
                }
            }
        }
        Ok(cells)
    }

    pub fn add_cell(&mut self, key: u64, h_ptr: u64) -> Result<usize, Box<dyn Error>> {
        let cell_size = KEY_SIZE + PTR_SIZE;
        let mut hdr = self.header()?;

        let c_ptr_offset = hdr.free_start;
        let c_offset = hdr.free_end;

        let end = c_offset as usize;
        let start = end
            .checked_sub(cell_size)
            // 4 bytes because we still haven't updated the cell pointer array
            // and this may lead to cell pointers and the cell collide when the cell pointer
            // for this cell is added in (free_start + 4).
            .filter(|&s| s >= c_ptr_offset as usize + 4)
            .ok_or("Page overflow")?;

        self.data[start..start + 8].copy_from_slice(&key.to_le_bytes());
        self.data[start + 8..end].clone_from_slice(&h_ptr.to_le_bytes());

        // now we can't just append the cell pointer at the free_start
        // we need to keep the cell pointer ITSELF to be sorted in the
        // cell pointers slot array. that'll require us to find the
        // insertion point in the cell pointers array and SHIFT every
        // cell pointer after the inserted cell pointer BY ONE.
        // This operation will cost O(N logN), where N is the order of the
        // Btree, log N is the binary search TC for searching insertion point,
        // and in worst case you would need to shift around N-1 (only for Leaf
        // nodes though as we know the first element will almost certainly never
        // be shifted as the key wouldn't even lie in the this node), else N on
        // internal/root node.

        let cell_ptr = CellPointer {
            cell_offset: start as u16,
            cell_size: cell_size as u16,
        };

        let n_slots = ((c_ptr_offset - HEADER_SIZE as u16) / 4);
        let mut lo = 0u16;
        let mut hi = n_slots;

        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let mid_slot = self.slot(mid);
            let mid_key = u64::from_le_bytes(
                self.data[mid_slot.cell_offset as usize
                    ..mid_slot.cell_offset as usize + mid_slot.cell_size as usize]
                    .try_into()
                    .unwrap(),
            );

            if mid_key < key {
                lo = mid + 1;
            } else if mid_key > key {
                hi = mid - 1;
            }
        }

        let insert_idx = lo as usize;
        let shift_from_start = insert_idx * 4 + HEADER_SIZE;
        let shift_from_end = (n_slots as usize * 4) - (HEADER_SIZE);

        self.data
            .copy_within(shift_from_start..shift_from_end, shift_from_start + 4);

        self.data[shift_from_start..shift_from_start + 2]
            .copy_from_slice(&cell_ptr.cell_offset.to_le_bytes());
        self.data[shift_from_start + 2..shift_from_start + 4]
            .copy_from_slice(&cell_ptr.cell_size.to_le_bytes());

        hdr.free_start += 4;
        hdr.free_end -= start as u16;
        hdr.serialize(&mut self.data[..HEADER_SIZE]);

        Ok(n_slots as usize + 1)
    }

    pub fn handle_overflow_cell(&mut self) {}

    pub fn header(&self) -> Result<PageHeader, Box<dyn Error>> {
        PageHeader::deserialize(&self.data[..HEADER_SIZE])
            .ok_or("Couldn't deserialize the header".into())
    }

    /// Returns the CellPointer for index i in the page
    fn slot(&self, i: u16) -> CellPointer {
        let off = HEADER_SIZE + i as usize * SLOT_SIZE;

        CellPointer {
            cell_offset: u16::from_le_bytes(self.data[off..off + 2].try_into().unwrap()),
            cell_size: u16::from_le_bytes(self.data[off + 2..off + 4].try_into().unwrap()),
        }
    }
}

/// starting offset (cell_offset) → \[Cell\] ← ending offset, 8 bytes
pub(super) struct CellPointer {
    pub cell_offset: u16,
    pub cell_size: u16,
}

/// Cell thats stored in a page and contains the actual data,
/// grows from back in a page, referenced by the cell pointers
/// IF an internal/root node, it references a child page
/// else a leaf node and contains the page offset in the heap
/// organized file of the data record.
///
/// In the heap file this contains the actual data record (not _records_, by design
/// just meant to store one data member of the data record).
pub(super) struct Cell {
    /// Key seperator, usually a specified PK (if not, it's not this module's concern).
    /// type u64, not a B+tree's concern for ensuring the type
    /// abstraction above are responsible for ensuring a valid u64 key
    pub key: u64,
    pub c_ptr: Option<u64>, // None for PageKind::Leaf, else page offset. 8 bytes
    pub h_ptr: Option<HeapPointer>, // only PageKind::Leaf have this.
}

pub(super) struct HeapPointer {
    pub index: u64,
}

pub(super) struct HeapPage {
    pub data: [u8; super::heap::PAGE_SIZE],
}

impl HeapPage {
    pub fn header(&self) -> Option<HeapHeader> {
        HeapHeader::deserialize(&self.data)
    }

    pub fn add_records(&mut self, data_records: Vec<Vec<u8>>) -> Result<(), Box<dyn Error>> {
        for record in data_records {
            self.add_cell(record)?;
        }

        Ok(())
    }

    pub fn add_cell(&mut self, data: Vec<u8>) -> Result<(), Box<dyn Error>> {
        let d_len = data.len();
        let Some(header) = self.header() else {
            return Err("Couldn't deserialize the header".into());
        };

        let cell_ptr_offset = header.free_start;
        let data_end_offset = header.free_end; // cell insertion point

        let end = data_end_offset as usize;

        // TODO: implement overflow pages.
        let start = end
            .checked_sub(d_len)
            .filter(|&s| s >= cell_ptr_offset as usize)
            .ok_or("Page overflow!")?;

        self.data[start..end].clone_from_slice(&data);

        let cell_ptr = CellPointer {
            cell_offset: start as u16,
            cell_size: d_len as u16,
        };

        let c_start = cell_ptr_offset as usize;
        self.data[c_start..c_start + 2].clone_from_slice(&cell_ptr.cell_offset.to_le_bytes());
        self.data[c_start + 2..c_start + 4].clone_from_slice(&cell_ptr.cell_size.to_le_bytes());

        Ok(())
    }
}
