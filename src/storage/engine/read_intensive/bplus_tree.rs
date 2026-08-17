use std::{error::Error, path::PathBuf};

use pager::Pager;

use header::PageKind;

pub(super) mod header;
pub(super) mod heap;
pub(super) mod pager;
pub(super) mod slotted_page;

pub(super) const PAGE_SIZE: usize = 4096;
pub(super) const HEADER_SIZE: usize = 8 + 2 + 2 + 1 + 1 + 8;
pub(super) const KEY_SIZE: usize = 8;
pub(super) const PTR_SIZE: usize = 8;

pub(super) const SLOT_SIZE: usize = 4; // cell_offset: u16 + cell_size: u16

/// max seperator keys in a page
const ORDER: usize = (PAGE_SIZE - HEADER_SIZE) / (KEY_SIZE + PTR_SIZE);
const DEGREE: usize = ORDER / 2;
const FANOUT: usize = ORDER + 1;

struct BplusTree {
    root_id: u64,
    pager: Pager,
}

impl BplusTree {
    pub fn new(tree_path: PathBuf, heap_path: PathBuf) -> Result<Self, Box<dyn Error>> {
        let mut pager = Pager::new(tree_path, heap_path)?;
        let root_id = pager.allocate();

        Ok(Self { root_id, pager })
    }

    /// Traverses the tree, finds the cell content in page
    /// i.e. the offset & size of the actual data in the heap file
    /// returns a &Vec<u8> of the data, the callers can transmute it.
    pub fn get(&self, key: u64) -> Result<Vec<Vec<u8>>, Box<dyn Error>> {
        let mut page_id = self.root_id;
        loop {
            let (cells, p_hdr) = {
                let page = self.pager.fetch(page_id);
                let p_hdr = page.header()?;
                (page.get_cells()?, p_hdr)
            };

            match cells.binary_search_by(|cell| cell.key.cmp(&key)) {
                Ok(i) => {
                    match p_hdr.page_ty {
                        // Found it!
                        PageKind::Leaf => {
                            let cell = cells.get(i).unwrap();
                            let page = self.pager.fetch(page_id);
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

    fn breadcrumbs(&self, key: u64) -> Result<BreadCumbs, Box<dyn Error>> {
        let mut breadcrumb: Vec<u64> = Vec::new();

        let mut page_id = self.root_id;

        loop {
            let (cells, p_hdr) = {
                let page = self.pager.fetch(page_id);
                let p_hdr = page.header()?;
                (page.get_cells()?, p_hdr)
            };

            breadcrumb.push(p_hdr.id);
            match cells.binary_search_by(|c| c.key.cmp(&key)) {
                Ok(i) => match p_hdr.page_ty {
                    PageKind::Leaf => {
                        return Ok(BreadCumbs {
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
                        return Ok(BreadCumbs {
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
struct BreadCumbs {
    breadcrumb: Vec<u64>,
    found: bool,
}

