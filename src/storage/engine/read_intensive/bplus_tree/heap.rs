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
        let header = HeapHeader::deserialize(&buf).ok_or("Couldn't deserialize the header")?;

        let cell_ptrs = self.get_cell_ptr(&buf, &header);
        for cell_ptr in cell_ptrs {
            let off = cell_ptr.cell_offset as usize;
            let size = cell_ptr.cell_size as usize;
            let mut data = vec![0u8; size];
            data.copy_from_slice(&buf[off..off + size]);
            data_records.push(data);
        }

        if header.has_overflow_page() {
            self.collect_overflow_records(header.ptr, data_records)?;
        }

        Ok(())
    }

    pub fn collect_overflow_records(
        &self,
        overflow_id: u64,
        data_records: &mut Vec<Vec<u8>>,
    ) -> Result<(), Box<dyn Error>> {
        let mut cur_id = overflow_id;
        loop {
            let hp = self.fetch(cur_id)?;
            let header = hp.header()?;
            let cell_ptrs = self.get_cell_ptr(&hp.data, &header);
            for cell_ptr in cell_ptrs {
                let start = cell_ptr.cell_offset as usize;
                let end = start + cell_ptr.cell_size as usize;
                let mut data_record = vec![];
                data_record.copy_from_slice(&hp.data[start..end]);
                data_records.push(data_record);
            }

            if header.has_overflow_page() {
                cur_id = header.ptr;
            } else {
                break;
            }
        }

        Ok(())
    }

    fn get_cell_ptr(&self, buf: &[u8], header: &HeapHeader) -> Vec<CellPointer> {
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

    pub fn allocate_primary(&mut self) -> (u64, HeapPage) {
        let id = self.allocate();
        let header = HeapHeader::new_primary(id);
        let mut page = HeapPage {
            data: [0u8; PAGE_SIZE],
        };
        header.serialize(&mut page.data[..HEADER_SIZE]);
        (id, page)
    }

    pub fn allocate_overflow(&mut self) -> (u64, HeapPage) {
        let id = self.allocate();
        let header = HeapHeader::new_overflow(id);
        let mut page = HeapPage {
            data: [0u8; PAGE_SIZE],
        };
        header.serialize(&mut page.data[..HEADER_SIZE]);
        (id, page)
    }

    /// Inserts the data records in the specified heap page.
    pub fn insert_records(
        &mut self,
        primary_page_id: u64,
        data_record: Vec<Vec<u8>>,
    ) -> Result<(), Box<dyn Error>> {
        // First tails till the end of overflow pages (if they exist)
        // check if theres enough space, if yes insert the record
        // else insert a new page and insert.
        let mut page = self.fetch(primary_page_id)?;
        let header = page.header()?;

        let mut current_page = page;
        let mut current_header = header;
        let mut current_id = primary_page_id;

        if current_header.is_overflow_page() {
            let (of_id, of_page, of_hdr) = self.find_tail(current_header.ptr)?;
            current_id = of_id;
            current_page = of_page;
            current_header = of_hdr;
        }

        for record in data_record {
            let d_len = record.len();
            let needed = d_len + SLOT_SIZE;

            if current_header.remaining_space() >= needed {
                current_page.add_cell(record)?;
                current_header = current_page.header()?;
            } else {
                // overflow occured create a new overflow page
                let (overflow_id, mut overflow_page) = self.allocate_overflow();

                current_header.set_has_overflow(overflow_id);
                current_header.serialize(&mut current_page.data[..HEADER_SIZE]);
                self.write_page(current_id, &current_page)?;

                overflow_page.add_cell(record)?;

                current_id = overflow_id;
                current_page = overflow_page;
                current_header = current_page.header()?;
            }
        }

        self.write_page(current_id, &current_page)?;
        Ok(())
    }

    pub fn write_page(&self, id: u64, page: &HeapPage) -> Result<(), Box<dyn Error>> {
        self.heap_file
            .write_all_at(&page.data, Self::page_offset(id));
        Ok(())
    }
    /// Follows the ptr till the page contains a overflow page
    fn find_tail(&self, id: u64) -> Result<(u64, HeapPage, HeapHeader), Box<dyn Error>> {
        let mut cur_id = id;
        let mut cur_page = self.fetch(id)?;
        let mut cur_hdr = cur_page.header()?;

        while cur_hdr.has_overflow_page() {
            cur_id = cur_hdr.ptr;
            cur_page = self.fetch(cur_id)?;
            cur_hdr = cur_page.header()?;
        }
        Ok((cur_id, cur_page, cur_hdr))
    }

    /// Returns the chain of ptr from primary to the last overflow page
    pub fn free_chain(&self, primary_id: u64) -> Result<Vec<u64>, Box<dyn Error>> {
        let mut chain = vec![primary_id];
        let page = self.fetch(primary_id)?;
        let mut hdr = page.header()?;

        while hdr.has_overflow_page() {
            let next_id = hdr.ptr;
            chain.push(next_id);
            let page = self.fetch(next_id)?;
            hdr = page.header()?;
        }

        Ok(chain)
    }

    pub fn add_cell(
        &mut self,
        page_id: u64,
        key: u64,
        h_ptr: u64,
    ) -> Result<usize, Box<dyn Error>> {
        // IF the page is about to overflow we have to allocate a new page,
        // mark the current page to indicate a it has an overflow child, update the `ptr`
        // to point to the page id of the new allocated page, and finally update the allocated page
        // to the new overflow'd page).
        let page = self.fetch(page_id)?;

        let p_hdr = page.header()?;

        while p_hdr.has_overflow_page() {}

        todo!()
    }

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
