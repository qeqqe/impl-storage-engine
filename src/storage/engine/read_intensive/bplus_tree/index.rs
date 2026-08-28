use std::{
    cell::{RefCell, RefMut},
    collections::HashMap,
    error::Error,
    os::unix::fs::FileExt,
};

use crate::storage::engine::read_intensive::bplus_tree::{PageKind, header::IndexHeader};

use super::{
    PAGE_SIZE,
    slotted_page::{Cell, Page},
};

pub(super) struct Index {
    pub index_file: std::fs::File,
    pub index_path: std::path::PathBuf,
    pub index_frames: RefCell<HashMap<u64, Page>>, // TODO: add eviction policy
    pub next_id: u64,
}

impl Index {
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

        self.index_file
            .write_all_at(&buf, Self::page_offset(id))
            .expect("Failed to write header to heap file");

        self.index_frames.get_mut().insert(id, Page { data: buf });

        id
    }

    pub fn fetch(&self, id: u64) -> RefMut<'_, Page> {
        if !self.index_frames.borrow().contains_key(&id) {
            let mut buf = [0u8; PAGE_SIZE];
            self.index_file
                .read_exact_at(&mut buf, Self::page_offset(id))
                .unwrap();
        }
        RefMut::map(self.index_frames.borrow_mut(), |f| f.get_mut(&id).unwrap())
    }

    pub fn flush(&mut self, id: u64) -> Result<(), Box<dyn Error>> {
        let frames = self.index_frames.borrow();
        let page = frames.get(&id).ok_or("Page not in frames")?;
        self.index_file
            .write_all_at(&page.data, Self::page_offset(id));
        Ok(())
    }

    pub fn flush_all(&mut self) -> Result<(), Box<dyn Error>> {
        let frames = self.index_frames.borrow();
        for (&id, page) in frames.iter() {
            self.index_file
                .write_all_at(&page.data, Self::page_offset(id));
        }
        Ok(())
    }

    fn page_offset(id: u64) -> u64 {
        id * PAGE_SIZE as u64
    }
}
