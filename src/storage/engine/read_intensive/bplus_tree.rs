#![allow(unused)]

use std::{error::Error, path::PathBuf};

use header::PageKind;
use pager::Pager;
use slotted_page::Cell;

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
            return Err("Overflow page not found!?".into());
        };

        // collecting all the cells in the logical page, no matter if the page is
        // overfull or not
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

        // Left cells are just remaning cells in the current overflow page
        // The promote cell is prompted to the parent, further pointing to right cells*
        // and left cells
        // Right cells are the split from original and form a new page which is pointed by the
        // promoted cell
        let left_cells = &overflow_cells[..split];
        let promote_cell = &overflow_cells[split];
        let right_cells = &overflow_cells[split + 1..];

        let right_page_id = self.pager.index.allocate();

        match overflow_hdr.page_ty {
            PageKind::Leaf => {
                // *On leaf level, the right cell's first element is the same as the promoted key
                let right_cells_with_promoted: Vec<_> = std::iter::once(promote_cell)
                    .chain(right_cells.iter())
                    .collect();

                {
                    let mut left_page = self.pager.index.fetch(overflow_page_id);
                    left_page.rebuild_from_cells(
                        left_cells,
                        PageKind::Leaf,
                        overflow_page_id,
                        right_page_id,
                    )?;
                }

                {
                    let mut right_page = self.pager.index.fetch(right_page_id);
                    let right_data: Vec<Cell> = right_cells_with_promoted
                        .into_iter()
                        .map(|c| Cell {
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
                        overflow_hdr.ptr,
                    )?;
                }
            }
            _ => {
                let promote_c_ptr = promote_cell
                    .c_ptr
                    .ok_or("Internal promote cell missing c_ptr")?;

                {
                    let mut left_page = self.pager.index.fetch(overflow_page_id);
                    left_page.rebuild_from_cells(
                        left_cells,
                        PageKind::Internal,
                        overflow_page_id,
                        promote_c_ptr,
                    )?;
                }

                {
                    let mut right_page = self.pager.index.fetch(right_page_id);
                    let right_data: Vec<Cell> = right_cells
                        .iter()
                        .map(|c| Cell {
                            key: c.key,
                            c_ptr: c.c_ptr,
                            h_ptr: None,
                        })
                        .collect();

                    right_page.rebuild_from_cells(
                        &right_data,
                        PageKind::Internal,
                        right_page_id,
                        overflow_hdr.ptr,
                    )?;
                }
            }
        };

        if let Some(&parent_id) = breadcrumbs.last() {
            let n_slot = {
                let mut parent_page = self.pager.index.fetch(parent_id);
                let parent_hdr = parent_page.header()?;

                let parent_cells = parent_page.get_cells()?;
                let fixed_cells: Vec<Cell> = parent_cells
                    .into_iter()
                    .map(|c| Cell {
                        key: c.key,
                        c_ptr: c.c_ptr.map(|p| {
                            if p == overflow_page_id {
                                right_page_id
                            } else {
                                p
                            }
                        }),
                        h_ptr: c.h_ptr,
                    })
                    .collect();

                let fixed_ptr = if parent_hdr.ptr == overflow_page_id {
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

                parent_page.add_cell(promote_cell.key, overflow_page_id)?
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
                root_page.add_cell(promote_cell.key, overflow_page_id)?;
            }

            {
                let mut left_page = self.pager.index.fetch(overflow_page_id);
                let hdr = left_page.header()?;
                if hdr.page_ty == PageKind::Root {
                    left_page.set_header_kind(PageKind::Internal)?;
                }
            }

            Ok(Some(new_root_id))
        }
    }

    fn delete(&mut self, key: u64) -> Result<(), Box<dyn Error>> {
        let crumbs = self.breadcrumbs(key)?;

        if !crumbs.found {
            return Err("Key not found".into());
        }

        let mut trail = crumbs.breadcrumb;

        let Some(leaf_id) = trail.pop() else {
            return Err("Empty breadcrumb trail".into());
        };

        let (removed_cell, remaining) = {
            let mut leaf_page = self.pager.index.fetch(leaf_id);
            let cells = leaf_page.get_cells()?;
            let idx = cells
                .binary_search_by(|c| c.key.cmp(&key))
                .map_err(|_| "Key not found in leaf page")?;

            let removed = leaf_page.remove_cell_at(idx)?;
            let n = leaf_page.num_cells()?;
            (removed, n)
        };

        if leaf_id == self.root_id {
            return Ok(());
        }

        // if there's no underfull just propogate the key up the tree
        if remaining >= DEGREE {
            self.propagate_key_update(key, leaf_id, &trail)?;
            return Ok(());
        }

        self.handle_underfull(leaf_id, &mut trail)?;

        Ok(())
    }

    fn propagate_key_update(
        &mut self,
        old_key: u64,
        child_id: u64,
        ancestors: &[u64],
    ) -> Result<(), Box<dyn Error>> {
        // we know that the left most cell's key of a leaf node will always
        // be the same as the parent node's key pointing to it. so if we find
        // any of the occurence of deleted key while propogating up the tree and we can
        // replace it with `new_first_key`
        let new_first_key = {
            let leaf_page = self.pager.index.fetch(child_id);
            let leaf_cells = leaf_page.get_cells()?;
            match leaf_cells.first() {
                Some(cell) => cell.key,
                None => return Ok(()),
            }
        };

        for &ancestor_id in ancestors.iter().rev() {
            let found_idx = {
                let page = self.pager.index.fetch(ancestor_id);
                let cells = page.get_cells()?;
                cells.binary_search_by(|c| c.key.cmp(&old_key)).ok()
            };

            if let Some(idx) = found_idx {
                let mut page = self.pager.index.fetch(ancestor_id);
                let hdr = page.header()?;
                let cells = page.get_cells()?;

                let updated_cells: Vec<Cell> = cells
                    .into_iter()
                    .enumerate()
                    .map(|(i, c)| {
                        if i == idx {
                            Cell {
                                key: new_first_key,
                                c_ptr: c.c_ptr,
                                h_ptr: c.h_ptr,
                            }
                        } else {
                            c
                        }
                    })
                    .collect();

                page.rebuild_from_cells(&updated_cells, hdr.page_ty, hdr.id, hdr.ptr)?;

                // there will only be ONE occurence at MAX so just
                // exit when you find one.
                return Ok(());
            }
        }

        Ok(())
    }

    fn handle_underfull(
        &mut self,
        underfull_id: u64,
        ancestors: &mut Vec<u64>,
    ) -> Result<(), Box<dyn Error>> {
        let Some(&parent_id) = ancestors.last() else {
            return Ok(());
        };

        let (parent_hdr, parent_cells) = {
            let parent_page = self.pager.index.fetch(parent_id);
            let hdr = parent_page.header()?;
            let cells = parent_page.get_cells()?;
            (hdr, cells)
        };

        let sibling = self.find_siblings(underfull_id, &parent_cells, &parent_hdr)?;

        let child_idx = sibling.child_idx;
        let left_sibling_id = sibling.left;
        let right_sibling_id = sibling.right;

        let underfull_cells = {
            let page = self.pager.index.fetch(underfull_id);
            page.get_cells()?
        };
        let underfull_hdr = {
            let page = self.pager.index.fetch(underfull_id);
            page.header()?
        };

        if let Some(left_id) = left_sibling_id {
            let left_cells = {
                let page = self.pager.index.fetch(left_id);
                page.get_cells()?
            };

            if left_cells.len() > DEGREE {
                return self.redistribute_from_left(
                    left_id,
                    underfull_id,
                    parent_id,
                    child_idx,
                    &underfull_hdr,
                );
            }

            if let Some(right_id) = right_sibling_id {
                let right_cells = {
                    let page = self.pager.index.fetch(right_id);
                    page.get_cells()?
                };

                if right_cells.len() > DEGREE {
                    return self.redistribute_from_right(
                        underfull_id,
                        right_id,
                        parent_id,
                        child_idx,
                        &underfull_hdr,
                    );
                }
            }
        }

        todo!()
    }
    /// this method finds the left and right siblings of a given child page in the parent page's cells.
    /// It returns the index of the child in the parent's cells, and the IDs of the left and right
    /// siblings if they exist...
    fn find_siblings(
        &self,
        child_id: u64,
        parent_cells: &[slotted_page::Cell],
        parent_hdr: &header::PageHeader,
    ) -> Result<Sibling, Box<dyn Error>> {
        let mut child_idx = parent_cells.len();

        for (i, cell) in parent_cells.iter().enumerate() {
            if cell.c_ptr == Some(child_id) {
                child_idx = i;
                break;
            }
        }

        if child_idx == parent_cells.len() && parent_hdr.ptr == child_id {
            let left = if parent_cells.is_empty() {
                None
            } else {
                let last = parent_cells.len() - 1;
                parent_cells[last].c_ptr
            };
            return Ok(Sibling {
                child_idx,
                left,
                right: None,
            });
        }

        let left = if child_idx > 0 {
            parent_cells[child_idx - 1].c_ptr
        } else {
            None
        };

        let right = if child_idx + 1 < parent_cells.len() {
            parent_cells[child_idx + 1].c_ptr
        } else if child_idx < parent_cells.len() {
            Some(parent_hdr.ptr)
        } else {
            None
        };

        Ok(Sibling {
            child_idx,
            left,
            right,
        })
    }

    fn redistribute_from_left(
        &mut self,
        left_id: u64,
        underfull_id: u64,
        parent_id: u64,
        child_idx: usize,
        underfull_hdr: &header::PageHeader,
    ) -> Result<(), Box<dyn Error>> {
        let is_leaf = underfull_hdr.page_ty == PageKind::Leaf;

        let (borrowed_cell, separator_idx) = {
            let mut left_page = self.pager.index.fetch(left_id);
            let left_cells = left_page.get_cells()?;
            let last_idx = left_cells.len() - 1;
            let cell = left_page.remove_cell_at(last_idx)?;
            (cell, child_idx - 1)
        };

        if is_leaf {
            {
                let mut underfull_page = self.pager.index.fetch(underfull_id);
                let value = borrowed_cell.h_ptr.as_ref().unwrap().index;
                underfull_page.add_cell(borrowed_cell.key, value)?;
            }

            let new_separator = {
                let underfull_page = self.pager.index.fetch(underfull_id);
                let cells = underfull_page.get_cells()?;
                cells
                    .first()
                    .ok_or("Empty underfull after redistribute")?
                    .key
            };

            {
                let mut parent_page = self.pager.index.fetch(parent_id);
                parent_page.remove_cell_at(separator_idx)?;
                parent_page.add_cell(new_separator, left_id)?;
            }
        } else {
            let parent_sep_key = {
                let parent_page = self.pager.index.fetch(parent_id);
                let parent_cells = parent_page.get_cells()?;
                parent_cells[separator_idx].key
            };

            let left_old_rightmost = {
                let left_page = self.pager.index.fetch(left_id);
                left_page.header()?.ptr
            };

            {
                let mut underfull_page = self.pager.index.fetch(underfull_id);

                let underfull_cells = underfull_page.get_cells()?;
                let all_cells: Vec<slotted_page::Cell> = std::iter::once(slotted_page::Cell {
                    key: parent_sep_key,
                    c_ptr: Some(left_old_rightmost),
                    h_ptr: None,
                })
                .chain(underfull_cells)
                .collect();

                underfull_page.rebuild_from_cells(
                    &all_cells,
                    underfull_hdr.page_ty,
                    underfull_id,
                    underfull_hdr.ptr,
                )?;
            }

            {
                let mut parent_page = self.pager.index.fetch(parent_id);
                parent_page.remove_cell_at(separator_idx)?;
                parent_page.add_cell(borrowed_cell.key, left_id)?;
            }

            {
                let mut left_page = self.pager.index.fetch(left_id);
                if let Some(c) = borrowed_cell.c_ptr {
                    left_page.set_header_ptr(c)?;
                }
            }
        }

        Ok(())
    }

    fn redistribute_from_right(
        &mut self,
        underfull_id: u64,
        right_id: u64,
        parent_id: u64,
        child_idx: usize,
        underfull_hdr: &header::PageHeader,
    ) -> Result<(), Box<dyn Error>> {
        let is_leaf = underfull_hdr.page_ty == PageKind::Leaf;

        let borrowed_cell = {
            let mut right_page = self.pager.index.fetch(right_id);
            right_page.remove_cell_at(0)?
        };

        let separator_idx = child_idx;

        if is_leaf {
            {
                let mut underfull_page = self.pager.index.fetch(underfull_id);
                let value = borrowed_cell.h_ptr.as_ref().unwrap().index;
                underfull_page.add_cell(borrowed_cell.key, value)?;
            }

            let new_separator = {
                let right_page = self.pager.index.fetch(right_id);
                let cells = right_page.get_cells()?;
                cells.first().ok_or("Empty right after redistribute")?.key
            };

            {
                let mut parent_page = self.pager.index.fetch(parent_id);
                parent_page.remove_cell_at(separator_idx)?;
                parent_page.add_cell(new_separator, underfull_id)?;
            }
        } else {
            let parent_sep_key = {
                let parent_page = self.pager.index.fetch(parent_id);
                let parent_cells = parent_page.get_cells()?;
                parent_cells[separator_idx].key
            };

            let borrowed_left_child = borrowed_cell.c_ptr.ok_or("Missing c_ptr")?;

            {
                let mut underfull_page = self.pager.index.fetch(underfull_id);
                let old_ptr = underfull_page.header()?.ptr;
                underfull_page.add_cell(parent_sep_key, old_ptr)?;
                underfull_page.set_header_ptr(borrowed_left_child)?;
            }

            {
                let mut parent_page = self.pager.index.fetch(parent_id);
                parent_page.remove_cell_at(separator_idx)?;
                parent_page.add_cell(borrowed_cell.key, underfull_id)?;
            }
        }

        Ok(())
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

struct Sibling {
    child_idx: usize,
    left: Option<u64>,
    right: Option<u64>,
}
