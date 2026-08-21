use std::{
    cell::{RefCell, RefMut},
    collections::HashMap,
    error::Error,
    os::unix::fs::FileExt,
};

use crate::storage::engine::read_intensive::bplus_tree::header::PageHeader;

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
    pub fn allocate(&mut self) -> u64 {
        let id = self.next_id;
        // TODO: introduce a better way to find next
        // available id then just incrementing
        self.next_id += 1;
        self.index_frames.get_mut().insert(
            id,
            Page {
                data: [0u8; PAGE_SIZE],
            },
        );

        id
    }

    /// the `Page` itself doensn't have the abilitiy to pull the
    /// overflow page, so we will let Pager do the work to handle the
    /// overflow pages...
    pub fn get_cells(&self, id: u64) -> Result<Vec<Cell>, Box<dyn Error>> {
        let mut p_id = id;
        let mut cells = Vec::new();

        let mut page = self.fetch(p_id);
        let p_hdr = page.header()?;

        while p_hdr.has_overflow_page() {
            cells.extend(page.get_cells()?);
            p_id = p_hdr.ptr;
            page = self.fetch(p_id);
        }

        Ok(cells)
    }

    pub fn rightmost_non_overflow_ptr(&self, id: u64) -> Result<u64, Box<dyn Error>> {
        let mut p_hdr = self.fetch_header(id)?;
        while p_hdr.has_overflow_page() {
            p_hdr = self.fetch_header(p_hdr.ptr)?;
        }

        Ok(p_hdr.ptr)
    }

    // TODO: this this really faster?
    // we are invoking a disk i/o instead of referring the in memeory page.
    pub fn fetch_header(&self, id: u64) -> Result<PageHeader, Box<dyn Error>> {
        let mut buf = [0u8; super::HEADER_SIZE];

        self.index_file
            .read_exact_at(&mut buf, Self::page_offset(id));

        match PageHeader::deserialize(&buf) {
            Some(p_hdr) => Ok(p_hdr),
            None => Err("Couldn't parse the header".into()),
        }
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

    fn page_offset(id: u64) -> u64 {
        id * PAGE_SIZE as u64
    }
}
