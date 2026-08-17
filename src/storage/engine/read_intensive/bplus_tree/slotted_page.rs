use std::error::Error;

use super::{
    HEADER_SIZE, PAGE_SIZE, SLOT_SIZE,
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
                    let h_offset =
                        u64::from_le_bytes(self.data[start + 8..start + 16].try_into().unwrap());
                    let size =
                        u64::from_le_bytes(self.data[start + 16..start + 24].try_into().unwrap());
                    cells.push(Cell {
                        key,
                        c_ptr: None,
                        h_ptr: Some(HeapPointer { h_offset, size }),
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

    pub fn fetch_heap_data(
        &self,
        cell: &Cell,
        heap_file: &std::fs::File,
    ) -> Result<&Vec<u8>, Box<dyn Error>> {
        todo!()
    }
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
struct CellPointer {
    cell_offset: u16,
    cell_size: u16,
}

pub(super) struct Cell {
    /// Key seperator, usually a specified PK (if not, it's not this module's concern).
    /// type u64, not a B+tree's concern for ensuring the type
    /// abstraction above are responsible for ensuring a valid u64 key
    pub key: u64,
    pub c_ptr: Option<u64>, // None for PageKind::Leaf, else page offset. 8 bytes
    pub h_ptr: Option<HeapPointer>, // only PageKind::Leaf have this.
}

struct HeapPointer {
    h_offset: u64,
    size: u64,
}
