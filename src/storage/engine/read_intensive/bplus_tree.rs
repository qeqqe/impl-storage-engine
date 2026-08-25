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
        let Some(overfull_page_id) = breadcrumbs.pop() else {
            return Err("Overfull page not found!?".into());
        };

        let (overfull_hdr, overfull_cells) = {
            let page = self.pager.index.fetch(overfull_page_id);
            let hdr = page.header()?;
            let cells = page.get_cells()?;
            (hdr, cells)
        };

        let n = overfull_cells.len();
        // keep these many elements remanining in the current page,
        // (split + 1)th element will be promoted to the parent node and further
        // will have the rest of the nodes as childs.
        let split = n.div_ceil(2);

        let left_cells = &overfull_cells[..split];
        let promote_cell = &overfull_cells[split];
        let right_cells = &overfull_cells[split + 1..];

        let right_page_id = self.pager.index.allocate();

        match overfull_hdr.page_ty {
            PageKind::Leaf => {
                let right_cells_with_promoted: Vec<_> = std::iter::once(promote_cell)
                    .chain(right_cells.iter())
                    .collect();

                {
                    let mut left_page = self.pager.index.fetch(overfull_page_id);
                    left_page.rebuild_from_cells(
                        left_cells,
                        PageKind::Leaf,
                        overfull_page_id,
                        right_page_id,
                    )?;
                }

                {
                    let mut right_page = self.pager.index.fetch(right_page_id);
                    let right_data: Vec<slotted_page::Cell> = right_cells_with_promoted
                        .into_iter()
                        .map(|c| slotted_page::Cell {
                            key: c.key,
                            c_ptr: c.c_ptr,
                            h_ptr: c
                                .h_ptr
                                .as_ref()
                                .map(|h| slotted_page::HeapPointer { index: h.index }),
                        })
                        .collect();
                    right_page.rebuild_from_cells(
                        &right_data,
                        PageKind::Leaf,
                        right_page_id,
                        overfull_hdr.ptr,
                    )?;
                }
            }
            _ => {
                let promote_c_ptr = promote_cell
                    .c_ptr
                    .ok_or("Internal promote cell missing c_ptr")?;

                {
                    let mut left_page = self.pager.index.fetch(overfull_page_id);
                    left_page.rebuild_from_cells(
                        left_cells,
                        PageKind::Internal,
                        overfull_page_id,
                        promote_c_ptr,
                    )?;
                }

                {
                    let mut right_page = self.pager.index.fetch(right_page_id);
                    let right_data: Vec<slotted_page::Cell> = right_cells
                        .iter()
                        .map(|c| slotted_page::Cell {
                            key: c.key,
                            c_ptr: c.c_ptr,
                            h_ptr: None,
                        })
                        .collect();

                    right_page.rebuild_from_cells(
                        &right_data,
                        PageKind::Internal,
                        right_page_id,
                        overfull_hdr.ptr,
                    )?;
                }
            }
        };

        if let Some(&parent_id) = breadcrumbs.last() {
            let n_slot = {
                let mut parent_page = self.pager.index.fetch(parent_id);
                let parent_hdr = parent_page.header()?;

                let parent_cells = parent_page.get_cells()?;
                let fixed_cells: Vec<slotted_page::Cell> = parent_cells
                    .into_iter()
                    .map(|c| slotted_page::Cell {
                        key: c.key,
                        c_ptr: c.c_ptr.map(|p| {
                            if p == overfull_page_id {
                                right_page_id
                            } else {
                                p
                            }
                        }),
                        h_ptr: c.h_ptr,
                    })
                    .collect();

                let fixed_ptr = if parent_hdr.ptr == overfull_page_id {
                    right_page_id
                } else {
                    parent_hdr.ptr
                };

                parent_page.rebuild_from_cells(
                    &fixed_cells,
                    parent_hdr.page_ty,
                    parent_id,
                    fixed_ptr,
                )?;

                parent_page.add_cell(promote_cell.key, overfull_page_id)?
            };

            if n_slot > ORDER {
                self.handle_overfull(breadcrumbs)
            } else {
                Ok(None)
            }
        } else {
            let new_root_id = self.pager.index.allocate();
            {
                let mut root_page = self.pager.index.fetch(new_root_id);
                root_page.init_header(new_root_id, PageKind::Root, right_page_id);
                root_page.add_cell(promote_cell.key, overfull_page_id)?;
            }

            {
                let mut left_page = self.pager.index.fetch(overfull_page_id);
                let hdr = left_page.header()?;
                if hdr.page_ty == PageKind::Root {
                    left_page.set_header_kind(PageKind::Internal)?;
                }
            }

            Ok(Some(new_root_id))
        }
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
