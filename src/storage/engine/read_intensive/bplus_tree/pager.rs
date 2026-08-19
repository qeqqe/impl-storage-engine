use std::{collections::HashMap, error::Error, path::PathBuf};

use super::{heap::Heap, index::Index, slotted_page::Cell};

pub(super) struct Pager {
    pub index: Index,
    pub heap: Heap,
}

impl Pager {
    pub fn new(index_path: PathBuf, heap_path: PathBuf) -> Result<Self, Box<dyn Error>> {
        let index_file = std::fs::File::open(&index_path)?;
        let heap_file = std::fs::File::open(&heap_path)?;
        let heap = Heap {
            heap_file,
            path: heap_path,
            next_id: 0,
        };
        let index = Index {
            index_file,
            index_path,
            index_frames: HashMap::new().into(),
            next_id: 0,
        };

        Ok(Pager { index, heap })
    }

    pub fn fetch_heap_data(
        &self,
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
}
