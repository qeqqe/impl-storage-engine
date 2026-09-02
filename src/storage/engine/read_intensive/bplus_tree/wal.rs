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

use std::error::Error;
use std::fs::{File, OpenOptions};
use std::os::unix::fs::FileExt;
use std::path::PathBuf;

use super::{buffer_pool::BufferPool, header::WAL_HEADER_SIZE, header::WalHeader};

pub const WAL_RECORD_HEADER_SIZE: usize = 8 + 8 + 8 + 1 + 8 + 8 + 4 + 4; // 41 bytes

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordType {
    Begin = 1,
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

#[derive(Debug, Clone)]
pub struct WalRecordHeader {
    pub lsn: u64,
    pub prev_lsn: u64,
    pub txn_id: u64,
    pub record_type: RecordType,
    pub page_id: u64,
    pub undo_next_lsn: u64,
    pub payload_len: u32,
    pub crc32: u32,
}

impl WalRecordHeader {
    pub fn serialize(&self, buf: &mut [u8]) {
        buf[0..8].copy_from_slice(&self.lsn.to_le_bytes());
        buf[8..16].copy_from_slice(&self.prev_lsn.to_le_bytes());
        buf[16..24].copy_from_slice(&self.txn_id.to_le_bytes());
        buf[24] = self.record_type as u8;
        buf[25..33].copy_from_slice(&self.page_id.to_le_bytes());
        buf[33..41].copy_from_slice(&self.undo_next_lsn.to_le_bytes());
        buf[41..45].copy_from_slice(&self.payload_len.to_le_bytes());
        buf[45..49].copy_from_slice(&self.crc32.to_le_bytes());
    }

    pub fn deserialize(buf: &[u8]) -> Option<Self> {
        Some(Self {
            lsn: u64::from_le_bytes(buf[0..8].try_into().ok()?),
            prev_lsn: u64::from_le_bytes(buf[8..16].try_into().ok()?),
            txn_id: u64::from_le_bytes(buf[16..24].try_into().ok()?),
            record_type: RecordType::try_from(buf[24]).ok()?,
            page_id: u64::from_le_bytes(buf[25..33].try_into().ok()?),
            undo_next_lsn: u64::from_le_bytes(buf[33..41].try_into().ok()?),
            payload_len: u32::from_le_bytes(buf[41..45].try_into().ok()?),
            crc32: u32::from_le_bytes(buf[45..49].try_into().ok()?),
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

        let header = WalHeader {
            last_checkpoint_lsn: 0,
            next_lsn: 0,
            next_txn_id: 0,
        };

        let mut buf = [0u8; WAL_HEADER_SIZE];
        header.serialize(&mut buf);

        log_file.write_all_at(&buf, 0)?;

        Ok(Self {
            log_file,
            log_path,
            header,
        })
    }
}
