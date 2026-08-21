#![allow(unused)]

use std::{error::Error, path::PathBuf};

use pager::Pager;

use header::PageKind;

pub(super) mod header;
pub(super) mod heap;
pub(super) mod index;
pub(super) mod pager;
pub(super) mod slotted_page;

pub(super) const PAGE_SIZE: usize = 4096;
pub(super) const HEADER_SIZE: usize = 8 + 2 + 2 + 1 + 1 + 8;
pub(super) const KEY_SIZE: usize = 8;
pub(super) const PTR_SIZE: usize = 8;

pub(super) const SLOT_SIZE: usize = 4; // cell_offset: u16 + cell_size: u16

/// max seperator keys in a page
///
/// `KEY_SIZE` + `PTR_SIZE` + `SLOT_SIZE` because for a single entry of a node
/// there will be a cell pointer (`SLOT_SIZE`, 4 bytes) and the actual cell
/// `KEY_SIZE` (the page id) `u64` 8 bytes AND `PTR_SIZE` `u64` 8 bytes  
/// (ptr to the child node for internal nodes OR heap file page index for leaf nodes).
const ORDER: usize = (PAGE_SIZE - HEADER_SIZE) / (KEY_SIZE + PTR_SIZE + SLOT_SIZE);
const FANOUT: usize = ORDER + 1;
const DEGREE: usize = FANOUT / 2;

struct BplusTree {
    root_id: u64,
    pager: Pager,
}

impl BplusTree {
    pub fn new(tree_path: PathBuf, heap_path: PathBuf) -> Result<Self, Box<dyn Error>> {
        let mut pager = Pager::new(tree_path, heap_path)?;
        let root_id = pager.index.allocate();

        Ok(Self { root_id, pager })
    }

    /// Traverses the tree, finds the cell content in page
    /// i.e. the offset & size of the actual data in the heap file
    /// returns a &Vec<u8> of the data, the callers can transmute it.
    pub fn get(&self, key: u64) -> Result<Vec<Vec<u8>>, Box<dyn Error>> {
        let mut page_id = self.root_id;
        loop {
            let (cells, p_hdr) = {
                let page = self.pager.index.fetch(page_id);
                let p_hdr = page.header()?;
                (page.get_cells()?, p_hdr)
            };

            match cells.binary_search_by(|cell| cell.key.cmp(&key)) {
                Ok(i) => {
                    match p_hdr.page_ty {
                        // Found it!
                        PageKind::Leaf => {
                            let cell = cells.get(i).unwrap();
                            let page = self.pager.index.fetch(page_id);
                            let mut data_records: Vec<Vec<u8>> = vec![vec![]];
                            self.pager.fetch_heap_data(cell, &mut data_records)?;
                            return Ok(data_records);
                        }
                        // Navigataion internal/root node
                        _ => {
                            let child_page_id = if i < cells.len() {
                                cells.get(i).unwrap().c_ptr.unwrap()
                            } else {
                                p_hdr.id
                            };

                            page_id = child_page_id;
                        }
                    };
                }
                Err(i) => {
                    if p_hdr.page_ty == PageKind::Leaf {
                        return Err("Entry not found".into());
                    }

                    let child_page_id = if i < cells.len() {
                        cells.get(i).unwrap().c_ptr.unwrap()
                    } else {
                        p_hdr.ptr
                    };
                    page_id = child_page_id;
                }
            }
        }
    }

    // TODO: update the data records to contain more metadata about the
    // inserted item for a better labled addressing of the data memebers,
    // updates can mess up things if we're not careful withi it.
    fn insert(&mut self, key: u64, data_records: Vec<Vec<u8>>) -> Result<(), Box<dyn Error>> {
        let mut breadcrumbs = self.breadcrumbs(key)?;

        if breadcrumbs.found {
            return Err("Key already exists".into());
        }

        let mut breadcrumbs = breadcrumbs.breadcrumb;

        let Some(index_page_id) = breadcrumbs.pop() else {
            return Err("No page found".into());
        };

        // we first have to insert the data record into the heap file then we
        // can have the offset of the data record that we can add to the cell's of
        // index file current page
        let heap_page_id = self.pager.heap.allocate();
        let mut heap_page = self.pager.heap.fetch(heap_page_id)?;
        heap_page.add_records(data_records)?;

        // now we can store the cell in the index page itself.
        let n_slot = {
            let mut page = self.pager.index.fetch(index_page_id);
            page.add_cell(key, heap_page_id)?
        };

        if n_slot > ORDER {
            // index page was popped off so we need to re-insert
            breadcrumbs.push(index_page_id);
            match self.handle_overfull(&mut breadcrumbs)? {
                Some(new_id) => self.root_id = new_id,
                None => return Ok(()),
            }
        }

        Ok(())
    }

    fn handle_overfull(
        &mut self,
        breadcrumbs: &mut Vec<u64>,
    ) -> Result<Option<u64>, Box<dyn Error>> {
        let Some(overflow_page_id) = breadcrumbs.pop() else {
            return Err("Overflowed page now found!?".into());
        };

        let (overflow_hdr, overflow_cells) = {
            let page = self.pager.index.fetch(overflow_page_id);
            let hdr = page.header()?;
            let cells = page.get_cells()?;
            (hdr, cells)
        };

        let n = overflow_cells.len();
        // keep these many elements remanining in the current page,
        // (split + 1)th element will be promoted to the parent node and further
        // will have the rest of the nodes as childs.
        let split = n.div_ceil(2);

        todo!()
    }

    fn breadcrumbs(&self, key: u64) -> Result<BreadCrumbs, Box<dyn Error>> {
        let mut breadcrumb: Vec<u64> = Vec::new();

        let mut page_id = self.root_id;

        loop {
            let (cells, p_hdr) = {
                let page = self.pager.index.fetch(page_id);
                let p_hdr = page.header()?;
                (page.get_cells()?, p_hdr)
            };

            breadcrumb.push(p_hdr.id);
            match cells.binary_search_by(|c| c.key.cmp(&key)) {
                Ok(i) => match p_hdr.page_ty {
                    PageKind::Leaf => {
                        return Ok(BreadCrumbs {
                            breadcrumb,
                            found: true,
                        });
                    }
                    _ => {
                        let child_page_id = if i < cells.len() {
                            cells.get(i).unwrap().c_ptr.unwrap()
                        } else {
                            p_hdr.ptr
                        };
                        page_id = child_page_id;
                    }
                },
                Err(i) => match p_hdr.page_ty {
                    PageKind::Leaf => {
                        return Ok(BreadCrumbs {
                            breadcrumb,
                            found: false,
                        });
                    }
                    _ => {
                        let child_page_id = if i < cells.len() {
                            cells.get(i).unwrap().c_ptr.unwrap()
                        } else {
                            p_hdr.ptr
                        };
                        page_id = child_page_id;
                    }
                },
            }
        }
    }
}
/// Stack that stores the page ids while traversing to leaf
struct BreadCrumbs {
    breadcrumb: Vec<u64>,
    found: bool,
}
