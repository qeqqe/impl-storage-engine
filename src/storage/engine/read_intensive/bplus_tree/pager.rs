use std::{error::Error, path::PathBuf};

use super::{buffer_pool::BufferPool, heap::Heap, index::Index, slotted_page::Cell};

pub(super) struct Pager {
    pub index: Index,
    pub heap: Heap,
}

impl Pager {
    pub fn new(index_path: PathBuf, heap_path: PathBuf) -> Result<Self, Box<dyn Error>> {
        let heap_file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&heap_path)?;

        let index_file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&index_path)?;

        let heap = Heap::new(heap_file, heap_path);
        let index = Index::new(index_file, index_path);

        Ok(Pager { index, heap })
    }

    pub fn fetch_heap_data(
        &mut self,
        cell: &Cell,
        data_record: &mut Vec<Vec<u8>>,
    ) -> Result<(), Box<dyn Error>> {
        if let Some(ref heap_ptr) = cell.h_ptr {
            self.heap.get_record(heap_ptr.index, data_record)?;
            Ok(())
        } else {
            Err("Couldn't find the header pointer".into())
        }
    }

    pub fn flush_all(&mut self) -> Result<(), Box<dyn Error>> {
        self.index.flush_all()?;
        self.heap.flush_all()?;
        Ok(())
    }

    pub fn discard_all_dirty(&mut self) {
        self.index.discard_dirty();
        self.heap.discard_dirty();
    }
}

