//! Private local startup credentials. Never expose values in diagnostics.
use std::{collections::BTreeMap, fmt, fs::File, io::Read, path::Path};

const MAX_BYTES: u64 = 16 * 1024;
const ALLOWED: [&str; 7] = [
    "AI_GATEWAY_API_KEY",
    "OPENROUTER_API_KEY",
    "OPENAI_API_KEY",
    "XAI_API_KEY",
    "FISH_API_KEY",
    "FISH_REFERENCE_ID",
    "TAVILY_API_KEY",
];

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct CredentialLoad {
    pub loaded: usize,
    pub existing: usize,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialError {
    Open,
    Permissions,
    TooLarge,
    Read,
    Syntax,
}
impl fmt::Display for CredentialError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Open => "Unable to open private credentials file",
            Self::Permissions => "Credentials file must be an owned regular file with mode 0600",
            Self::TooLarge => "Credentials file exceeds 16 KiB",
            Self::Read => "Unable to read private credentials file",
            Self::Syntax => "Invalid private credentials file syntax",
        })
    }
}
impl std::error::Error for CredentialError {}

/// Import supported provider credentials; existing environment entries win,
/// including existing empty entries. Missing file is a successful no-op.
///
/// # Safety
/// Call before spawning ANY application/runtime/Bevy threads, and ensure no
/// other thread or signal handler can read or modify the process environment
/// during this call. Rust 2024 environment mutation requires this guarantee.
pub unsafe fn load_startup(path: &Path) -> Result<CredentialLoad, CredentialError> {
    let Some(file) = open_private(path)? else {
        return Ok(CredentialLoad::default());
    };
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| CredentialError::Read)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(CredentialError::TooLarge);
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| CredentialError::Syntax)?;
    // Parse everything before mutating the process environment: syntax failure
    // cannot leave a partly imported configuration behind.
    let values = parse_credentials(text)?;
    let mut result = CredentialLoad::default();
    for (key, value) in values {
        if std::env::var_os(&key).is_some() {
            result.existing += 1;
        } else {
            // SAFETY: delegated to this function's documented caller contract.
            unsafe {
                std::env::set_var(key, value);
            }
            result.loaded += 1;
        }
    }
    Ok(result)
}
fn open_private(path: &Path) -> Result<Option<File>, CredentialError> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // NONBLOCK avoids hanging on a FIFO before the regular-file check.
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC);
    }
    let file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(CredentialError::Open),
    };
    let metadata = file.metadata().map_err(|_| CredentialError::Read)?;
    if !metadata.is_file() {
        return Err(CredentialError::Permissions);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: geteuid has no pointer arguments or mutable process state.
        if metadata.mode() & 0o777 != 0o600 || metadata.uid() != unsafe { libc::geteuid() } {
            return Err(CredentialError::Permissions);
        }
    }
    #[cfg(not(unix))]
    if std::fs::symlink_metadata(path)
        .map_err(|_| CredentialError::Read)?
        .file_type()
        .is_symlink()
    {
        return Err(CredentialError::Permissions);
    }
    if metadata.len() > MAX_BYTES {
        return Err(CredentialError::TooLarge);
    }
    Ok(Some(file))
}

/// Intentionally no interpolation or shell evaluation. Duplicate allowed keys
/// are rejected rather than resolving potentially ambiguous credentials.
fn parse_credentials(text: &str) -> Result<BTreeMap<String, String>, CredentialError> {
    if text.len() as u64 > MAX_BYTES {
        return Err(CredentialError::TooLarge);
    }
    if text.contains('\0') {
        return Err(CredentialError::Syntax);
    }
    let mut chars = text
        .strip_prefix('\u{feff}')
        .unwrap_or(text)
        .chars()
        .peekable();
    let mut values = BTreeMap::new();
    while chars.peek().is_some() {
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        if chars.peek().is_none() {
            break;
        }
        if chars.peek() == Some(&'#') {
            for c in chars.by_ref() {
                if c == '\n' {
                    break;
                }
            }
            continue;
        }
        let mut key = String::new();
        while chars
            .peek()
            .is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_')
        {
            key.push(chars.next().unwrap());
        }
        if key == "export" && chars.peek().is_some_and(|c| *c == ' ' || *c == '\t') {
            while chars.peek().is_some_and(|c| *c == ' ' || *c == '\t') {
                chars.next();
            }
            key.clear();
            while chars
                .peek()
                .is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_')
            {
                key.push(chars.next().unwrap());
            }
        }
        if key.is_empty() || key.starts_with(|c: char| c.is_ascii_digit()) {
            return Err(CredentialError::Syntax);
        }
        while chars.peek().is_some_and(|c| *c == ' ' || *c == '\t') {
            chars.next();
        }
        if chars.next() != Some('=') {
            return Err(CredentialError::Syntax);
        }
        while chars.peek().is_some_and(|c| *c == ' ' || *c == '\t') {
            chars.next();
        }
        let mut value = String::new();
        if let Some(quote @ ('\'' | '"')) = chars.peek().copied() {
            chars.next();
            let mut closed = false;
            while let Some(c) = chars.next() {
                if c == quote {
                    closed = true;
                    break;
                }
                if c == '\\' && quote == '"' {
                    value.push(match chars.next().ok_or(CredentialError::Syntax)? {
                        'n' => '\n',
                        'r' => '\r',
                        't' => '\t',
                        '\\' => '\\',
                        '"' => '"',
                        // No expansion: preserve a literal escaped dollar.
                        '$' => '$',
                        _ => return Err(CredentialError::Syntax),
                    });
                } else {
                    value.push(c);
                }
            }
            if !closed {
                return Err(CredentialError::Syntax);
            }
            while chars
                .peek()
                .is_some_and(|c| *c == ' ' || *c == '\t' || *c == '\r')
            {
                chars.next();
            }
            match chars.peek() {
                None | Some('\n') | Some('#') => (),
                _ => return Err(CredentialError::Syntax),
            }
            if chars.peek() == Some(&'#') {
                for c in chars.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
            } else if chars.peek() == Some(&'\n') {
                chars.next();
            }
        } else {
            while chars.peek().is_some_and(|c| *c != '\n' && *c != '\r') {
                value.push(chars.next().unwrap());
            }
            // A comment begins only after whitespace, preserving URL fragments.
            let comment = value
                .char_indices()
                .find(|(i, c)| *c == '#' && (*i == 0 || value[..*i].ends_with(char::is_whitespace)))
                .map(|(i, _)| i);
            if let Some(index) = comment {
                value.truncate(index);
            }
            value = value.trim_end().to_owned();
            if value.contains(['\'', '"']) {
                return Err(CredentialError::Syntax);
            }
        }
        if ALLOWED.contains(&key.as_str()) && values.insert(key, value).is_some() {
            return Err(CredentialError::Syntax);
        }
    }
    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn allowlist_quotes_comments_export_and_newlines() {
        let values = parse_credentials("# private\r\nexport AI_GATEWAY_API_KEY=\"fakekey\\nsecond\" # comment\r\nFISH_API_KEY='fakekey literal $HOME'\nFISH_REFERENCE_ID=fakekey#fragment\nIGNORED=not-imported\n").unwrap();
        assert_eq!(values.len(), 3);
        assert_eq!(values["AI_GATEWAY_API_KEY"], "fakekey\nsecond");
        assert_eq!(values["FISH_API_KEY"], "fakekey literal $HOME");
        assert_eq!(values["FISH_REFERENCE_ID"], "fakekey#fragment");
        assert!(!values.contains_key("IGNORED"));
        assert_eq!(
            parse_credentials("FISH_API_KEY=\"fakekey\nsecond\"\n").unwrap()["FISH_API_KEY"],
            "fakekey\nsecond"
        );
    }
    #[cfg(unix)]
    #[test]
    fn private_file_rejects_permissions_symlinks_and_size() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let directory =
            std::env::temp_dir().join(format!("waifu-credentials-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("credentials.env");
        std::fs::write(&path, "FISH_API_KEY=fakekey\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(
            open_private(&path),
            Err(CredentialError::Permissions)
        ));
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(open_private(&path).unwrap().is_some());
        let link = directory.join("link.env");
        symlink(&path, &link).unwrap();
        assert!(open_private(&link).is_err());
        std::fs::write(&path, vec![b'x'; MAX_BYTES as usize + 1]).unwrap();
        assert!(matches!(
            open_private(&path),
            Err(CredentialError::TooLarge)
        ));
        assert!(
            open_private(&directory.join("missing.env"))
                .unwrap()
                .is_none()
        );
        assert!(matches!(
            open_private(&directory),
            Err(CredentialError::Permissions)
        ));
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn malformed_input_never_includes_values_in_errors() {
        for text in [
            "FISH_API_KEY='fakekey",
            "FISH_API_KEY=\"fakekey\" trailing",
            "FISH_API_KEY=\"fakekey\\q\"",
            "FISH_API_KEY=fakekey\nFISH_API_KEY=fakekey",
            "FISH_API_KEY=fakekey\0",
            "1BAD=fakekey",
            "not an assignment",
        ] {
            let error = parse_credentials(text).err().unwrap();
            assert_eq!(error, CredentialError::Syntax);
            assert!(!error.to_string().contains("fakekey"));
        }
        assert_eq!(
            parse_credentials(&"x".repeat(16 * 1024 + 1)).err(),
            Some(CredentialError::TooLarge)
        );
        assert!(parse_credentials("FISH_API_KEY=\n").unwrap()["FISH_API_KEY"].is_empty());
    }

    #[test]
    fn all_supported_chat_provider_keys_load_from_the_private_profile() {
        let parsed = parse_credentials("AI_GATEWAY_API_KEY=gateway\nOPENROUTER_API_KEY=router\nOPENAI_API_KEY=openai\nXAI_API_KEY=xai\nFISH_API_KEY=fish\nFISH_REFERENCE_ID=voice\nTAVILY_API_KEY=tavily\nUNRELATED_TOKEN=ignored\n").unwrap();
        assert_eq!(parsed.len(), ALLOWED.len());
        for key in ALLOWED {
            assert!(parsed.contains_key(key));
        }
        assert!(!parsed.contains_key("UNRELATED_TOKEN"));
    }
}
