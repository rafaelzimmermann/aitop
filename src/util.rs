use std::io::Write;
use std::path::Path;

/// Cached tokens and panel snapshots live in an owner-only directory: a world-readable
/// cache dir would expose the fact (and contents) of an account token to other users.
#[cfg(unix)]
pub fn secret_dir(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::create_dir_all(dir);
    let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
}

#[cfg(not(unix))]
pub fn secret_dir(dir: &Path) {
    let _ = std::fs::create_dir_all(dir);
}

#[cfg(unix)]
pub fn write_secret(path: &Path, raw: &str) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(mut fh) = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)
    {
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        let _ = fh.write_all(raw.as_bytes());
    }
}

#[cfg(not(unix))]
pub fn write_secret(path: &Path, raw: &str) {
    let _ = std::fs::write(path, raw);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_dir_and_files_are_owner_only() {
        let dir = std::env::temp_dir().join("aitop-secret-dir-test");
        let _ = std::fs::remove_dir_all(&dir);
        secret_dir(&dir);
        write_secret(&dir.join("token.json"), "{\"access_token\":\"x\"}");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o700, "cache dir must not be group/world readable");
            let file_mode = std::fs::metadata(dir.join("token.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(file_mode, 0o600);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
