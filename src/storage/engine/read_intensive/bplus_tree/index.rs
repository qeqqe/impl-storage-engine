use std::{error::Error, fs::File, os::unix::fs::FileExt, path::PathBuf};

use crate::storage::engine::read_intensive::bplus_tree::{PageKind, header::IndexHeader};

use super::{PAGE_SIZE, buffer_pool::BufferPool, slotted_page::Page};

const INDEX_POOL_CAPACITY: usize = 10_000;

pub(super) struct Index {
    pub index_file: File,
    pub index_path: PathBuf,
    pub pool: BufferPool<Page>,
    pub next_id: u64,
}

impl Index {
    pub fn new(index_file: File, index_path: PathBuf) -> Self {
        let next_id = index_file
            .metadata()
            .map(|m| m.len() / PAGE_SIZE as u64)
            .unwrap_or(0);
        Index {
            index_file,
            index_path,
            pool: BufferPool::new(INDEX_POOL_CAPACITY),
            next_id,
        }
    }

    pub fn allocate(&mut self, page_ty: PageKind) -> u64 {
        let id = self.next_id;
        // TODO: introduce a better way to find next
        // available id then just incrementing
        let end_offset = Self::page_offset(id) + PAGE_SIZE as u64;
        self.index_file
            .set_len(end_offset)
            .expect("Failed to set length of heap file");
        self.next_id += 1;

        let header = IndexHeader {
            id,
            free_start: super::HEADER_SIZE as u16,
            free_end: PAGE_SIZE as u16,
            flags: 0,
            ptr: 0,
            page_ty,
        };

        let mut buf = [0u8; PAGE_SIZE];
        header.serialize(&mut buf);

        if let Some(evicted) = self
            .pool
            .insert(id, Page { data: buf })
            .expect("buf pool capacity exceeded during allocate... all pages pinned...)")
            && evicted.was_dirty
        {
            self.index_file
                .write_all_at(&evicted.page.data, Self::page_offset(evicted.id))
                .expect("Failed to write-back stolen page during allocate");
        }
        self.pool.mark_dirty(id);

        id
    }

    pub fn fetch(&mut self, id: u64) -> &Page {
        if !self.pool.contains(id) {
            let mut buf = [0u8; PAGE_SIZE];
            self.index_file
                .read_exact_at(&mut buf, Self::page_offset(id))
                .unwrap();
            if let Some(evicted) = self
                .pool
                .insert(id, Page { data: buf })
                .expect("buf pool capacity exceeded during index fetch... all pages pinned...")
                && evicted.was_dirty
            {
                self.index_file
                    .write_all_at(&evicted.page.data, Self::page_offset(evicted.id))
                    .expect("Failed to write-back stolen page during fetch");
            }
        }
        self.pool.get(id).unwrap()
    }

    pub fn fetch_mut(&mut self, id: u64) -> &mut Page {
        if !self.pool.contains(id) {
            let mut buf = [0u8; PAGE_SIZE];
            self.index_file
                .read_exact_at(&mut buf, Self::page_offset(id))
                .unwrap();
            if let Some(evicted) = self
                .pool
                .insert(id, Page { data: buf })
                .expect("buf pool capacity exceeded during index fetch_mut... all pages pinned...")
                && evicted.was_dirty
            {
                self.index_file
                    .write_all_at(&evicted.page.data, Self::page_offset(evicted.id))
                    .expect("Failed to write-back stolen page during fetch_mut");
            }
        }
        self.pool.mark_dirty(id);
        self.pool.get_mut(id).unwrap()
    }

    pub fn flush(&mut self, id: u64) -> Result<(), Box<dyn Error>> {
        if self.pool.is_dirty(id)
            && let Some(page) = self.pool.get(id)
        {
            let data = page.data;
            self.index_file.write_all_at(&data, Self::page_offset(id))?;
            self.pool.clear_dirty_single(id);
        }
        Ok(())
    }

    pub fn flush_all(&mut self) -> Result<(), Box<dyn Error>> {
        let dirty_ids = self.pool.dirty_page_ids();
        for id in dirty_ids {
            if let Some(page) = self.pool.get(id) {
                let data = page.data;
                self.index_file.write_all_at(&data, Self::page_offset(id))?;
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

    fn page_offset(id: u64) -> u64 {
        id * PAGE_SIZE as u64
    }
}
