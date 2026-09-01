// Im still not sure if i should go NO-STEAL as
// it'll defo use more memory then STEAL NO-FORCE
// as we are not allowed to flush the dirty pages until
// the transaction commited... idkk....

use std::collections::{HashMap, VecDeque};
use std::error::Error;

pub(super) struct BufferPool<P> {
    frames: HashMap<u64, FrameEntry<P>>,
    dirty: HashMap<u64, bool>,
    lru_order: VecDeque<u64>,
    max_frames: usize,
}

struct FrameEntry<P> {
    page: P,
    pin_count: u32,
}

impl<P> BufferPool<P> {
    pub fn new(max_frames: usize) -> Self {
        BufferPool {
            frames: HashMap::new(),
            dirty: HashMap::new(),
            lru_order: VecDeque::new(),
            max_frames,
        }
    }

    pub fn contains(&self, id: u64) -> bool {
        self.frames.contains_key(&id)
    }

    pub fn get(&self, id: u64) -> Option<&P> {
        self.frames.get(&id).map(|e| &e.page)
    }

    pub fn get_mut(&mut self, id: u64) -> Option<&mut P> {
        self.touch(id);
        self.frames.get_mut(&id).map(|e| &mut e.page)
    }

    pub fn insert(&mut self, id: u64, page: P) -> Result<(), Box<dyn Error>> {
        if !self.frames.contains_key(&id)
            && self.frames.len() >= self.max_frames
            && self.evict_one().is_none()
        {
            return Err("Buffer pool full: no clean unpinned pages available for eviction (NO-STEAL backpressure)".into());
        }

        self.frames.insert(id, FrameEntry { page, pin_count: 0 });
        self.lru_order.retain(|&x| x != id);
        self.lru_order.push_back(id);

        Ok(())
    }

    pub fn mark_dirty(&mut self, id: u64) {
        self.dirty.insert(id, true);
    }

    pub fn is_dirty(&self, id: u64) -> bool {
        self.dirty.get(&id).copied().unwrap_or(false)
    }

    pub fn drain_dirty(&mut self) -> Vec<(u64, &P)> {
        let dirty_ids: Vec<u64> = self.dirty.keys().copied().collect();
        let mut result = Vec::new();
        for id in &dirty_ids {
            if let Some(entry) = self.frames.get(id) {
                result.push((*id, &entry.page));
            }
        }
        result
    }

    pub fn clear_dirty(&mut self) {
        self.dirty.clear();
    }

    pub fn clear_dirty_single(&mut self, id: u64) {
        self.dirty.remove(&id);
    }

    pub fn pin(&mut self, id: u64) {
        if let Some(entry) = self.frames.get_mut(&id) {
            entry.pin_count += 1;
        }
    }

    pub fn unpin(&mut self, id: u64) {
        if let Some(entry) = self.frames.get_mut(&id) {
            entry.pin_count = entry.pin_count.saturating_sub(1);
        }
    }

    pub fn remove(&mut self, id: u64) -> Option<P> {
        self.dirty.remove(&id);
        self.lru_order.retain(|&x| x != id);
        self.frames.remove(&id).map(|e| e.page)
    }

    fn touch(&mut self, id: u64) {
        self.lru_order.retain(|&x| x != id);
        self.lru_order.push_back(id);
    }
    /// Dirty pages are NEVER evicted. Only clean, unpinned pages are candidates for LRU eviction
    fn evict_one(&mut self) -> Option<u64> {
        let mut evict_idx = None;
        for (i, &id) in self.lru_order.iter().enumerate() {
            let Some(entry) = self.frames.get(&id) else {
                continue;
            };
            if entry.pin_count == 0 && !self.is_dirty(id) {
                evict_idx = Some((i, id));
                break;
            }
        }

        if let Some((idx, id)) = evict_idx {
            self.lru_order.remove(idx);
            self.frames.remove(&id);
            self.dirty.remove(&id);
            Some(id)
        } else {
            None
        }
    }

    pub fn dirty_page_ids(&self) -> Vec<u64> {
        self.dirty.keys().copied().collect()
    }

    pub fn all_page_ids(&self) -> Vec<u64> {
        self.frames.keys().copied().collect()
    }

    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn no_steal_refuses_to_evict_dirty_pages() {
        let mut pool: BufferPool<Vec<u8>> = BufferPool::new(2);

        pool.insert(1, vec![1, 1, 1]).unwrap();
        pool.mark_dirty(1);

        pool.insert(2, vec![2, 2, 2]).unwrap();
        pool.mark_dirty(2);

        // pool is full of dirty pages.. inserting a 3rd must fail cus NO-STEAL
        let err = pool.insert(3, vec![3, 3, 3]);
        assert!(err.is_err());
        assert!(pool.contains(1));
        assert!(pool.contains(2));
        assert!(!pool.contains(3));

        // after cleaning page 1 the eviction should succeed
        pool.clear_dirty_single(1);
        assert!(pool.insert(3, vec![3, 3, 3]).is_ok());

        assert!(!pool.contains(1));
        assert!(pool.contains(2));
        assert!(pool.contains(3));
    }

    #[test]
    fn no_steal_refuses_to_evict_pinned_pages() {
        let mut pool: BufferPool<Vec<u8>> = BufferPool::new(2);

        pool.insert(1, vec![1, 1, 1]).unwrap();
        pool.pin(1);

        pool.insert(2, vec![2, 2, 2]).unwrap();
        pool.pin(2);

        // here both the pages are clean, BUT pinned... so they can't evict
        let err = pool.insert(3, vec![3, 3, 3]);
        assert!(err.is_err());

        // Unpin page 1; now it can be evicted
        pool.unpin(1);
        assert!(pool.insert(3, vec![3, 3, 3]).is_ok());
        assert!(!pool.contains(1));
        assert!(pool.contains(2));
        assert!(pool.contains(3));
    }
}
