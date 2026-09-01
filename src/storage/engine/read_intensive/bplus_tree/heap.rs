//! This is an interface for accessing/modifying the heap organized file.
//! This implements the slotted page heap file, basically the slotted pages
//! on index file but instead of containing key/child_ptr this holds the actual
//! data record. all the actual data offsets of the page will be organized by the
//! b+ trees's disk persisted index file and will map the corresponding data
//! record in heap through the slotted page's cell Header Pointer.

// TODO: implement overflow pages cus they are bound to exist

use std::fs::File;
use std::{error::Error, os::unix::fs::FileExt};

use crate::storage::engine::read_intensive::bplus_tree::header::HEAP_HEADER_SIZE;

use super::buffer_pool::BufferPool;
use super::slotted_page::HeapPage;

use super::{header::HeapHeader, slotted_page::CellPointer};

pub(super) const PAGE_SIZE: usize = 8192;
pub(super) const HEADER_SIZE: usize = 21;
pub(super) const SLOT_SIZE: usize = 4;

const HEAP_POOL_CAPACITY: usize = 10_000;

pub(super) struct Heap {
    pub heap_file: File,
    pub path: std::path::PathBuf,
    pub next_id: u64,
    pub pool: BufferPool<HeapPage>,
}

impl Heap {
    pub fn new(heap_file: File, path: std::path::PathBuf) -> Self {
        Heap {
            heap_file,
            path,
            next_id: 0,
            pool: BufferPool::new(HEAP_POOL_CAPACITY),
        }
    }
    pub fn get_record(
        &mut self,
        id: u64,
        data_records: &mut Vec<Vec<u8>>,
    ) -> Result<(), Box<dyn Error>> {
        let page = self.fetch(id)?;
        let header =
            HeapHeader::deserialize(&page.data).ok_or("Couldn't deserialize the header")?;

        let cell_ptrs = Self::get_cell_ptr(&page.data, &header);
        for cell_ptr in &cell_ptrs {
            let off = cell_ptr.cell_offset as usize;
            let size = cell_ptr.cell_size as usize;
            let mut data = vec![0u8; size];
            data.copy_from_slice(&page.data[off..off + size]);
            data_records.push(data);
        }

        let has_overflow = header.has_overflow_page();
        let overflow_ptr = header.ptr;

        if has_overflow {
            self.collect_overflow_records(overflow_ptr, data_records)?;
        }

        Ok(())
    }

    pub fn collect_overflow_records(
        &mut self,
        overflow_id: u64,
        data_records: &mut Vec<Vec<u8>>,
    ) -> Result<(), Box<dyn Error>> {
        let mut cur_id = overflow_id;
        loop {
            let page = self.fetch(cur_id)?;
            let header = page.header()?;
            let cell_ptrs = Self::get_cell_ptr(&page.data, &header);
            for cell_ptr in cell_ptrs {
                let start = cell_ptr.cell_offset as usize;
                let end = start + cell_ptr.cell_size as usize;
                let mut data_record = vec![0u8; end - start];
                data_record.copy_from_slice(&page.data[start..end]);
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

    fn get_cell_ptr(buf: &[u8], header: &HeapHeader) -> Vec<CellPointer> {
        let range = (header.free_start - HEADER_SIZE as u16) / 4; // cell offset + cell size = 4 bytes
        // NOTE: here we can derive that a single cellpointer is a
        // data member's pointer of a row.
        let mut cell_ptr: Vec<CellPointer> = Vec::with_capacity(range as usize);

        for i in 0..range {
            cell_ptr.push(Self::slot(i, buf));
        }

        cell_ptr
    }

    pub fn fetch(&mut self, id: u64) -> Result<&HeapPage, Box<dyn Error>> {
        if !self.pool.contains(id) {
            let page = self.read_page_from_disk(id)?;
            self.pool.insert(id, page)?;
        }
        self.pool
            .get(id)
            .ok_or("Page not in pool after fetch".into())
    }

    pub fn fetch_mut(&mut self, id: u64) -> Result<&mut HeapPage, Box<dyn Error>> {
        if !self.pool.contains(id) {
            let page = self.read_page_from_disk(id)?;
            self.pool.insert(id, page)?;
        }
        self.pool.mark_dirty(id);
        self.pool
            .get_mut(id)
            .ok_or("Page not in pool after fetch_mut".into())
    }

    pub fn allocate(&mut self) -> u64 {
        let id = self.next_id;

        let end_offset = Self::page_offset(id) + PAGE_SIZE as u64;
        self.heap_file
            .set_len(end_offset)
            .expect("Failed to set length of heap file");

        let header = HeapHeader {
            id,
            ptr: 0,
            free_start: HEAP_HEADER_SIZE as u16,
            free_end: PAGE_SIZE as u16,
            flags: 0,
        };
        let mut page = HeapPage {
            data: [0u8; PAGE_SIZE],
        };
        header.serialize(&mut page.data[..HEADER_SIZE]);

        self.pool
            .insert(id, page)
            .expect("Buffer pool capacity exceeded during allocate (NO-STEAL backpressure)");
        self.pool.mark_dirty(id);

        self.next_id += 1;
        id
    }

    pub fn allocate_primary(&mut self) -> u64 {
        let id = self.allocate();
        let page = self.pool.get_mut(id).unwrap();
        let header = HeapHeader::new_primary(id);
        header.serialize(&mut page.data[..HEADER_SIZE]);
        self.pool.mark_dirty(id);
        id
    }

    pub fn allocate_overflow(&mut self) -> u64 {
        let id = self.allocate();
        let page = self.pool.get_mut(id).unwrap();
        let header = HeapHeader::new_overflow(id);
        header.serialize(&mut page.data[..HEADER_SIZE]);
        self.pool.mark_dirty(id);
        id
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
        let header = {
            let page = self.fetch(primary_page_id)?;
            page.header()?
        };

        let mut current_id = primary_page_id;

        if header.is_overflow_page() {
            current_id = self.find_tail_id(header.ptr)?;
        }

        for record in data_record {
            let d_len = record.len();
            let needed = d_len + SLOT_SIZE;

            let remaining = {
                let page = self.fetch(current_id)?;
                page.header()?.remaining_space()
            };

            if remaining >= needed {
                let page = self.fetch_mut(current_id)?;
                page.add_cell(record)?;
            } else {
                let overflow_id = self.allocate_overflow();

                {
                    let current_page = self.fetch_mut(current_id)?;
                    let mut hdr = current_page.header()?;
                    hdr.set_has_overflow(overflow_id);
                    hdr.serialize(&mut current_page.data[..HEADER_SIZE]);
                }

                {
                    let overflow_page = self.fetch_mut(overflow_id)?;
                    overflow_page.add_cell(record)?;
                }

                current_id = overflow_id;
            }
        }

        Ok(())
    }

    fn read_page_from_disk(&self, id: u64) -> Result<HeapPage, Box<dyn Error>> {
        let mut buf = [0u8; PAGE_SIZE];
        self.heap_file
            .read_exact_at(&mut buf, Self::page_offset(id))?;
        Ok(HeapPage { data: buf })
    }

    pub fn flush(&mut self, id: u64) -> Result<(), Box<dyn Error>> {
        if self.pool.is_dirty(id)
            && let Some(page) = self.pool.get(id)
        {
            let data = page.data;
            self.heap_file.write_all_at(&data, Self::page_offset(id))?;
            self.pool.clear_dirty_single(id);
        }
        Ok(())
    }

    pub fn flush_all(&mut self) -> Result<(), Box<dyn Error>> {
        let dirty_ids = self.pool.dirty_page_ids();
        for id in dirty_ids {
            if let Some(page) = self.pool.get(id) {
                let data = page.data;
                self.heap_file.write_all_at(&data, Self::page_offset(id))?;
            }
        }
        self.pool.clear_dirty();
        Ok(())
    }

    pub fn discard_dirty(&mut self) {
        let dirty_ids = self.pool.dirty_page_ids();
        for id in dirty_ids {
            self.pool.remove(id);
        }
    }

    fn find_tail_id(&mut self, id: u64) -> Result<u64, Box<dyn Error>> {
        let mut cur_id = id;
        loop {
            let hdr = {
                let page = self.fetch(cur_id)?;
                page.header()?
            };
            if hdr.has_overflow_page() {
                cur_id = hdr.ptr;
            } else {
                return Ok(cur_id);
            }
        }
    }

    /// returns the chain of ptr from primary to the last overflow page
    pub fn free_chain(&mut self, primary_id: u64) -> Result<Vec<u64>, Box<dyn Error>> {
        let mut chain = vec![primary_id];
        let mut cur_id = primary_id;
        loop {
            let hdr = {
                let page = self.fetch(cur_id)?;
                page.header()?
            };
            if hdr.has_overflow_page() {
                cur_id = hdr.ptr;
                chain.push(cur_id);
            } else {
                break;
            }
        }
        Ok(chain)
    }

    fn slot(i: u16, buf: &[u8]) -> CellPointer {
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
