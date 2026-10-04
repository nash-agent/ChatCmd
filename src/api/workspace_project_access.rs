use std::path::Path;

use axum::http::StatusCode;
use sqlx::{Sqlite, Transaction};

use super::{Problem, db_problem};

pub(super) fn approved_path(path: &str, enabled: bool) -> Result<Option<String>, Problem> {
    if !enabled {
        return Ok(None);
    }
    let invalid = || {
        Problem::new(
            StatusCode::BAD_REQUEST,
            "Invalid shared project folder",
            "Choose an existing absolute directory before allowing access in all conversations.",
        )
    };
    let requested = Path::new(path);
    if !requested.is_absolute() {
        return Err(invalid());
    }
    let canonical = requested.canonicalize().map_err(|_| invalid())?;
    if !canonical.is_dir() {
        return Err(invalid());
    }
    Ok(Some(canonical.to_string_lossy().into_owned()))
}

// A saved safe-read grant may include shared project paths. Conservatively invalidate
// all reusable grants when the local UI changes project access; normal policy still applies.
pub(super) async fn revoke_grants(
    transaction: &mut Transaction<'_, Sqlite>,
    now: i64,
) -> Result<(), Problem> {
    sqlx::query("UPDATE approval_grants SET state='revoked',updated_at_ms=? WHERE state='active'")
        .bind(now)
        .execute(&mut **transaction)
        .await
        .map_err(db_problem)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_is_explicit_and_bounded() {
        assert_eq!(approved_path("missing", false).unwrap(), None);
        assert!(approved_path("relative", true).is_err());
        let filesystem_root = std::env::current_dir()
            .unwrap()
            .canonicalize()
            .unwrap()
            .ancestors()
            .last()
            .unwrap()
            .to_path_buf();
        assert_eq!(
            approved_path(filesystem_root.to_str().unwrap(), true).unwrap(),
            Some(filesystem_root.to_string_lossy().into_owned())
        );
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().to_str().unwrap();
        assert_eq!(
            approved_path(path, true).unwrap(),
            Some(
                directory
                    .path()
                    .canonicalize()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            )
        );
        let file = directory.path().join("file");
        std::fs::write(&file, "data").unwrap();
        assert!(approved_path(file.to_str().unwrap(), true).is_err());
    }
}
