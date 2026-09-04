//! The WAL will give every Log record a log sequence number (LSN) and update
//! the modified data page's LSN in buffer pool with the matching the log
//! record's LSN number.
//!
//! On how this will be affecting the flushing of data page in buffer pools:
//!
//! Here with STEAL policy we will follow a strict rule...
//! Before the buffer pool manager issues write() for a data page, it checks:
//!
//! `Data_Page_LSN <= Last_Flushed_LSN_WAL`
//!
//! Then only can it be flushed to the file.
//!
//! If Last_Flushed_LSN_WAL is only at 1000, and the data page thats about to
//! be evicted/flushed is 1010 the buffer pool manager pauses the data page
//! flush and then does a Forced Log Flush; The engine forces a flush/fsync of
//! the WAL log buffer up to at least LSN 1010 to disk. Last_Flushed_LSN_WAL
//! becomes 1010.
//!
//! Now that Data_Page_LSN <= Last_Flushed_LSN_WAL, the buffer pool is finally
//! allowed to write the dirty data page to disk.
//!
//! Basically the ARIES algorithm

use std::error::Error;
use std::fs::{File, OpenOptions};
use std::os::unix::fs::FileExt;
use std::path::PathBuf;

use crate::storage::engine::read_intensive::bplus_tree::PageKind;
use crate::storage::engine::read_intensive::bplus_tree::wal_buffer::WalBuffer;

use super::{buffer_pool::BufferPool, header::WAL_HEADER_SIZE, header::WalHeader};

pub const WAL_RECORD_HEADER_SIZE: usize = 8 + 4 + 8 + 1 + 1 + 8 + 4 + 2; // 36 bytes
pub const WAL_POOL_CAPACITY: usize = 10_000;

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordType {
    Begin = 1, // the transaction begining
    Update = 2,
    Commit = 3,
    Abort = 4,
    /// Compensation log records are written during the undo phase of recovery,
    /// it writes the operations being performed, this is crucial cus we can be
    /// stuck in an infinte loop if the system crashes again while recovery.
    Clr = 5,
    Checkpoint = 6,
}

impl TryFrom<u8> for RecordType {
    type Error = u8;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Begin),
            2 => Ok(Self::Update),
            3 => Ok(Self::Commit),
            4 => Ok(Self::Abort),
            5 => Ok(Self::Clr),
            6 => Ok(Self::Checkpoint),
            _ => Err(value),
        }
    }
}

pub struct WalRecordHeader {
    pub lsn: u64,

    // pub offset: u32, // we don't need offset, we can derive it
    // pub prev_lsn: u64, // we don't need prev_lsn, we can derive it by self.lsn - 1
    /// This will allow track back (end->start) in WAL,
    /// we can derive the offset of the previous wal record by simply computing
    /// `current_offset - current.prev_record_len`
    pub prev_record_len: u32,
    pub txn_id: u64,
    pub record_type: RecordType,
    pub page_id: u64,
    pub is_index: bool, // else heap
    // [wal_record_header] \
    // [undo_data_size:u16][undo_payload_bytes] \
    // [redo_data_size:u16][redo_payload_bytes] \
    // [next_wal_record_header]
    pub payload_len: u32,         // +4 size for undo & redo size
    pub payload_page_offset: u16, // offset of the modified data record in the data page
}

impl WalRecordHeader {
    pub fn serialize(&self, buf: &mut [u8]) {
        buf[0..8].copy_from_slice(&self.lsn.to_le_bytes());
        buf[8..12].copy_from_slice(&self.prev_record_len.to_le_bytes());
        buf[12..20].copy_from_slice(&self.txn_id.to_le_bytes());
        buf[20] = self.record_type as u8;
        buf[21] = self.is_index as u8;
        buf[22..30].copy_from_slice(&self.page_id.to_le_bytes());
        buf[30..34].copy_from_slice(&self.payload_len.to_le_bytes());
        buf[34..36].copy_from_slice(&self.payload_page_offset.to_le_bytes());
    }

    pub fn deserialize(buf: &[u8]) -> Option<Self> {
        Some(Self {
            lsn: u64::from_le_bytes(buf[0..8].try_into().ok()?),
            prev_record_len: u32::from_le_bytes(buf[8..12].try_into().ok()?),
            txn_id: u64::from_le_bytes(buf[12..20].try_into().ok()?),
            record_type: RecordType::try_from(buf[20]).ok()?,
            is_index: buf[21] == 1,
            page_id: u64::from_le_bytes(buf[22..30].try_into().ok()?),
            payload_len: u32::from_le_bytes(buf[30..34].try_into().ok()?),
            payload_page_offset: u16::from_le_bytes(buf[34..36].try_into().ok()?),
        })
    }
}

#[derive(Debug, Clone)]
pub struct UpdatePayload<'a> {
    pub offset_in_page: u16,
    pub undo_data: &'a [u8], // Old bytes
    pub redo_data: &'a [u8], // New bytes
}

impl<'a> UpdatePayload<'a> {
    pub fn serialize(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&self.offset_in_page.to_le_bytes());
        buf.extend_from_slice(&(self.undo_data.len() as u16).to_le_bytes());
        buf.extend_from_slice(self.undo_data);
        buf.extend_from_slice(&(self.redo_data.len() as u16).to_le_bytes());
        buf.extend_from_slice(self.redo_data);
    }
}

pub(super) struct Wal {
    pub log_file: File,
    pub log_path: PathBuf,
    pub wal_buffer: WalBuffer,
    pub header: WalHeader,
}

impl Wal {
    pub fn new(log_path: PathBuf) -> Result<Self, Box<dyn Error>> {
        let log_file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&log_path)?;

        let existing_len = log_file.metadata()?.len();

        let header = if existing_len == 0 {
            // fresh WAL, never initialized
            let h = WalHeader {
                last_checkpoint_lsn: 0,
                last_wal_offset: WAL_HEADER_SIZE as u64,
                last_wal_len: 0,
                next_lsn: 0,
            };
            let mut buf = [0u8; WAL_HEADER_SIZE];
            h.serialize(&mut buf);
            log_file.write_all_at(&buf, 0)?;
            h
        } else if existing_len >= WAL_HEADER_SIZE as u64 {
            // existing WAL
            let mut buf = [0u8; WAL_HEADER_SIZE];
            log_file.read_exact_at(&mut buf, 0)?;
            WalHeader::deserialize(&buf).ok_or("corrupt wal header")?
        } else {
            // 1..WAL_HEADER_SIZE bytes; torn/corrupted
            return Err("wal file exists but header is truncated/corrupt".into());
        };

        let wal_buffer = WalBuffer {
            buffer: Vec::new(),
            offsets: Vec::new(),
            max_frames: WAL_POOL_CAPACITY,
        };

        Ok(Self {
            log_file,
            log_path,
            wal_buffer,
            header,
        })
    }

    pub fn write_record(
        &mut self,
        txn_id: u64,
        record_type: RecordType,
        page_id: u64,
        is_index: bool,
        payload: UpdatePayload,
    ) -> Result<u64, Box<dyn Error>> {
        let lsn = self.header.next_lsn;
        self.header.next_lsn += 1;

        let total_payload_len = payload.undo_data.len() + payload.redo_data.len() + 4;

        let mut buf: Vec<u8> = Vec::with_capacity(WAL_RECORD_HEADER_SIZE + total_payload_len);

        let rec_offset = self.header.last_wal_offset + self.header.last_wal_len as u64;

        let wal_rec_header = WalRecordHeader {
            lsn,
            prev_record_len: self.header.last_wal_len,
            txn_id,
            record_type,
            page_id,
            is_index,
            payload_len: total_payload_len as u32,
            payload_page_offset: payload.offset_in_page,
        };
        let mut wal_rec_buff = [0u8; WAL_RECORD_HEADER_SIZE];
        wal_rec_header.serialize(&mut wal_rec_buff);

        buf.extend_from_slice(&wal_rec_buff);
        buf.extend_from_slice(&(payload.undo_data.len() as u16).to_le_bytes());
        buf.extend_from_slice(payload.undo_data);
        buf.extend_from_slice(&(payload.redo_data.len() as u16).to_le_bytes());
        buf.extend_from_slice(payload.redo_data);

        // TODO: make this atomic
        self.header.last_wal_len = (total_payload_len + WAL_RECORD_HEADER_SIZE) as u32;
        self.header.last_wal_offset = rec_offset;

        self.wal_buffer.insert(&buf, lsn, rec_offset as usize);

        Ok(lsn)
    }
}
