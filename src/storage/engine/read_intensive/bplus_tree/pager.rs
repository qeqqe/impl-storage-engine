use std::{
    cell::{RefCell, RefMut},
    collections::HashMap,
    error::Error,
    os::unix::fs::FileExt,
    path::PathBuf,
};

use super::{PAGE_SIZE, slotted_page::Page};

pub(super) struct Pager {
    pub tree_file: std::fs::File,
    pub heap_file: std::fs::File,
    pub frames: RefCell<HashMap<u64, Page>>, // TODO: add eviction policy
    pub path_buf: std::path::PathBuf,
    pub next_available_id: u64,
}

impl Pager {
    pub fn new(tree_path: PathBuf, heap_path: PathBuf) -> Result<Self, Box<dyn Error>> {
        let tree_file = std::fs::File::open(&tree_path)?;
        let heap_file = std::fs::File::open(&heap_path)?;
        Ok(Pager {
            tree_file,
            heap_file,
            frames: HashMap::new().into(),
            path_buf: tree_path,
            next_available_id: 0,
        })
    }

    pub fn allocate(&mut self) -> u64 {
        let id = self.next_available_id;
        // TODO: introduce a better way to find next
        // available id then just incrementing
        self.next_available_id += 1;
        self.frames.get_mut().insert(
            id,
            Page {
                data: [0u8; PAGE_SIZE],
            },
        );

        id
    }

    pub fn fetch(&self, id: u64) -> RefMut<'_, Page> {
        if !self.frames.borrow().contains_key(&id) {
            let mut buf = [0u8; PAGE_SIZE];
            self.tree_file
                .read_exact_at(&mut buf, Self::page_offset(id))
                .unwrap();
        }
        RefMut::map(self.frames.borrow_mut(), |f| f.get_mut(&id).unwrap())
    }

    fn page_offset(id: u64) -> u64 {
        id * PAGE_SIZE as u64
    }
}
