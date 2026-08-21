//! This is an interface for accessing/modifying the heap organized file.
//! This implements the slotted page heap file, basically the slotted pages
//! on index file but instead of containing key/child_ptr this holds the actual
//! data record. all the actual data offsets of the page will be organized by the
//! b+ trees's disk persisted index file and will map the corresponding data
//! record in heap through the slotted page's cell Header Pointer.

// TODO: implement overflow pages cus they are bound to exist

use std::{error::Error, os::unix::fs::FileExt};

use super::slotted_page::HeapPage;

use super::{header::HeapHeader, slotted_page::CellPointer};

pub(super) const PAGE_SIZE: usize = 8192;
pub(super) const HEADER_SIZE: usize = 13;
pub(super) const SLOT_SIZE: usize = 4;

pub(super) struct Heap {
    pub heap_file: std::fs::File,
    pub path: std::path::PathBuf,
    pub next_id: u64,
}

impl Heap {
    pub fn get_record(
        &self,
        id: u64,
        data_records: &mut Vec<Vec<u8>>,
    ) -> Result<(), Box<dyn Error>> {
        let mut buf = [0u8; PAGE_SIZE];
        self.heap_file
            .read_exact_at(&mut buf, Self::page_offset(id))?;

        if let Some(header) = HeapHeader::deserialize(&buf) {
            let cell_ptrs = self.get_cell_ptr(&buf, header);
            for cell_ptr in cell_ptrs {
                let mut data = vec![];
                let off = cell_ptr.cell_offset as usize;
                let size = cell_ptr.cell_size as usize;
                data.extend_from_slice(&buf[off..off + size]);
                data_records.push(data);
            }
            Ok(())
        } else {
            Err("Couldn't deserialize the header".into())
        }
    }

    fn get_cell_ptr(&self, buf: &[u8], header: HeapHeader) -> Vec<CellPointer> {
        let range = (header.free_start - HEADER_SIZE as u16) / 4; // cell offset + cell size = 4 bytes
        // NOTE: here we can derive that a single cellpointer is a
        // data member's pointer of a row.
        let mut cell_ptr: Vec<CellPointer> = Vec::with_capacity(range as usize);

        for i in 0..range {
            cell_ptr.push(self.slot(i, buf));
        }

        cell_ptr
    }

    pub fn fetch(&self, id: u64) -> Result<HeapPage, Box<dyn Error>> {
        let mut buf = [0u8; PAGE_SIZE];
        self.heap_file
            .read_exact_at(&mut buf, Self::page_offset(id))?;
        Ok(HeapPage { data: buf })
    }

    pub fn allocate(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        // TODO: Add caching??
        id
    }

    // pub fn add_cell(
    //     &mut self,
    //     page_id: u64,
    //     key: u64,
    //     h_ptr: u64,
    // ) -> Result<usize, Box<dyn Error>> {
    //     // IF the page is about to overflow we have to allocate a new page,
    //     // mark the current page to indicate a it has an overflow child, update the `ptr`
    //     // to point to the page id of the new allocated page, and finally update the allocated page
    //     // header's `ptr` to contain the sibling/child point (the ptr that the page contained who now points
    //     // to the new overflow'd page).
    //  let page = self.fetch(page_id);
    //
    //
    //
    //     let p_hdr = page.header()?;
    //
    //     while p_hdr.has_overflow_page() {}
    //     todo!()
    // }

    fn slot(&self, i: u16, buf: &[u8]) -> CellPointer {
        let off = HEADER_SIZE + i as usize * SLOT_SIZE;

        CellPointer {
            cell_offset: u16::from_le_bytes(buf[off..off + 2].try_into().unwrap()),
            cell_size: u16::from_le_bytes(buf[off + 2..off + 4].try_into().unwrap()),
        }
    }

    fn page_offset(id: u64) -> u64 {
        PAGE_SIZE as u64 * id
    }
}
