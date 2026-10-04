use super::super::{FileIdentity, io_error, reject_reparse_metadata};
use crate::{DurabilityMode, RuntimeError, RuntimeResult};
use std::{fs, os::windows::ffi::OsStrExt as _, path::Path, time::Duration};
use windows_sys::Win32::Storage::FileSystem::{
    MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
};

pub(super) fn replace(
    temporary: tempfile::NamedTempFile,
    target: &Path,
    durability: DurabilityMode,
) -> RuntimeResult<()> {
    // Closing our own handle is necessary before Windows can rename the staged file.
    // TempPath retains cleanup ownership until publication succeeds or fails.
    let temporary = temporary.into_temp_path();
    let parent = target.parent().ok_or_else(|| {
        RuntimeError::new("invalid_path", "replacement destination has no parent")
    })?;
    let metadata = fs::symlink_metadata(target).map_err(io_error)?;
    reject_reparse_metadata(&metadata)?;
    let identity = FileIdentity::from_metadata(&metadata);
    let readonly = metadata.permissions().readonly();
    let parent_metadata = fs::symlink_metadata(parent).map_err(io_error)?;
    reject_reparse_metadata(&parent_metadata)?;
    let parent_identity = FileIdentity::from_metadata(&parent_metadata);
    let source: Vec<u16> = temporary.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = target.as_os_str().encode_wide().chain(Some(0)).collect();
    let flags = MOVEFILE_REPLACE_EXISTING
        | if durability == DurabilityMode::None {
            0
        } else {
            MOVEFILE_WRITE_THROUGH
        };

    for attempt in 0..=20 {
        if attempt > 0 {
            // A retry must not overwrite an edit or follow a replaced/reparse parent.
            validate_unchanged(target, &identity)?;
            validate_unchanged(parent, &parent_identity)?;
        }
        // SAFETY: both buffers remain valid NUL-terminated UTF-16 strings during the call.
        if unsafe { MoveFileExW(source.as_ptr(), destination.as_ptr(), flags) } != 0 {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        // Windows reports denied delete sharing as ACCESS_DENIED (5), as well as
        // SHARING_VIOLATION (32) / LOCK_VIOLATION (33). Never remove the target first.
        if readonly || attempt == 20 || !matches!(error.raw_os_error(), Some(5 | 32 | 33)) {
            return Err(io_error(error));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    unreachable!("bounded replacement loop always returns")
}

fn validate_unchanged(path: &Path, identity: &FileIdentity) -> RuntimeResult<()> {
    let metadata = fs::symlink_metadata(path).map_err(io_error)?;
    reject_reparse_metadata(&metadata)?;
    if FileIdentity::from_metadata(&metadata) != *identity {
        return Err(RuntimeError::new(
            "path_changed_after_authorization",
            "filesystem entry changed while waiting for Windows file replacement",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs::OpenOptions, io::Write as _, os::windows::fs::OpenOptionsExt as _};

    fn fixture() -> (
        tempfile::TempDir,
        std::path::PathBuf,
        tempfile::NamedTempFile,
    ) {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("target.py");
        fs::write(&target, b"original").unwrap();
        let mut staged = tempfile::NamedTempFile::new_in(directory.path()).unwrap();
        staged.write_all(b"replacement").unwrap();
        (directory, target, staged)
    }

    fn deny_delete_share(target: &Path) -> fs::File {
        OpenOptions::new()
            .read(true)
            .share_mode(3)
            .open(target)
            .unwrap()
    }

    #[test]
    fn windows_replace_recovers_when_delete_sharing_lock_is_released() {
        let (_directory, target, staged) = fixture();
        let handle = deny_delete_share(&target);
        let releaser = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            drop(handle);
        });
        replace(staged, &target, DurabilityMode::Data).unwrap();
        releaser.join().unwrap();
        assert_eq!(fs::read(target).unwrap(), b"replacement");
    }

    #[test]
    fn windows_replace_persistent_lock_preserves_original_and_cleans_staging() {
        let (directory, target, staged) = fixture();
        let _handle = deny_delete_share(&target);
        assert!(replace(staged, &target, DurabilityMode::Data).is_err());
        assert_eq!(fs::read(target).unwrap(), b"original");
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn windows_replace_does_not_clobber_changes_during_retry() {
        let (_directory, target, staged) = fixture();
        let handle = deny_delete_share(&target);
        let changed_target = target.clone();
        let changer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            fs::write(changed_target, b"concurrent edit must survive").unwrap();
            drop(handle);
        });
        let result = replace(staged, &target, DurabilityMode::Data);
        changer.join().unwrap();
        assert!(result.is_err());
        assert_eq!(fs::read(target).unwrap(), b"concurrent edit must survive");
    }
}
