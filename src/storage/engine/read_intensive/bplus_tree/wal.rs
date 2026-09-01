use std::fs::File;
use std::path::PathBuf;

#[repr(u8)]
#[derive(PartialEq, Eq, Clone, Copy)]
pub(super) enum WalOp {
    Undo = 0, // do we need this? i dont think so we are
    // using NO-STEAL, NO-FORCE under no circumstances will we
    // ever need to undo the changes cus the uncommited data will be dropped right as the system
    // crashes
    Redo = 1,
    Commit = 2,
    Abort = 3,
    Checkpoint = 4,
}

pub(super) struct WalHeader {
    pub last_checkpoint_lsn: u64,
    pub next_lsn: u64,
    pub next_txn_id: u64,
}

pub(super) struct Wal {
    pub log_file: File,
    pub log_path: PathBuf,
    pub header: WalHeader,
}
