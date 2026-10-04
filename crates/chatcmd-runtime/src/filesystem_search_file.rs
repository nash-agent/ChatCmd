use super::{FileScanState, WorkspaceService};
use crate::{RuntimeError, RuntimeResult};
use std::{
    fs::File,
    io::{BufReader, Seek, SeekFrom},
};

impl FileScanState {
    pub(super) fn suspend(&mut self) {
        // byte_offset counts consumed raw bytes, excluding BufReader read-ahead.
        // Queued matches/context remain in memory; the OS file handle does not.
        self.reader = None;
    }

    pub(super) fn resume(&mut self, workspace: &WorkspaceService) -> RuntimeResult<()> {
        if self.reader.is_some() {
            return Ok(());
        }
        // Re-authorize the path before reopening; a cursor must not follow a
        // replaced parent or reparse point outside the current workspace.
        let authorized = workspace.existing(&self.path)?;
        authorized.revalidate()?;
        let mut file = File::open(&*authorized).map_err(super::super::io_error)?;
        let metadata = file.metadata().map_err(super::super::io_error)?;
        if super::super::FileIdentity::from_metadata(&metadata) != self.identity {
            return Err(RuntimeError::new(
                "cursor_stale",
                "search file changed between pages; restart search",
            ));
        }
        authorized.revalidate()?;
        file.seek(SeekFrom::Start(self.byte_offset))
            .map_err(super::super::io_error)?;
        self.reader = Some(BufReader::with_capacity(64 * 1024, file));
        Ok(())
    }
}
