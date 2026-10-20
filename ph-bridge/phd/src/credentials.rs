use rand::RngCore;
use std::{fs, io::Write, path::Path};

/// Create a token once, without replacing a token used by running clients.
/// The default directory is private to its owner on Unix. On Windows it lives
/// under the user's LocalAppData directory and inherits that user's ACL.
pub fn load_or_create(path: &Path) -> std::io::Result<String> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    if !parent.exists() {
        fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
        }
    }
    if !path.exists() {
        // Publish only a complete credential. Concurrent starts either install
        // their token atomically or read the winner's existing token.
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        let mut bytes = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut bytes);
        let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        writeln!(file, "{token}")?;
        file.as_file().sync_all()?;
        match file.persist_noclobber(path) {
            Ok(_) => {}
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.error),
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if fs::metadata(path)?.permissions().mode() & 0o077 != 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "phd token file must have mode 0600",
            ));
        }
    }
    let token = fs::read_to_string(path)?.trim().to_owned();
    if token.len() < 32 {
        return Err(std::io::Error::other(
            "phd token must contain at least 32 characters",
        ));
    }
    Ok(token)
}

pub(crate) fn matches(expected: &str, actual: &str) -> bool {
    // Do not reveal a matching prefix through early returns.
    let mut difference = expected.len() ^ actual.len();
    for (a, b) in expected.bytes().zip(actual.bytes()) {
        difference |= usize::from(a ^ b);
    }
    difference == 0
}
