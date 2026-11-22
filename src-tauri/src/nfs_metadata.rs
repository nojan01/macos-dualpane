//! Recognize macOS AppleDouble companions, not arbitrary `._*` user files.
use std::ffi::OsStr;
use std::io::{self, Read};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

pub(crate) fn companion_owner(path: &Path) -> Option<PathBuf> {
    let name = path.file_name()?.as_bytes().strip_prefix(b"._")?;
    if name.is_empty() || name == b"." || name == b".." {
        return None;
    }
    Some(path.with_file_name(OsStr::from_bytes(name)))
}

pub(crate) fn is_macos_appledouble(path: &Path) -> io::Result<bool> {
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_file() {
        return Ok(false);
    }
    // Do not follow a replaced symlink or block on a replaced FIFO. Fail closed
    // on I/O errors: no destructive preview is returned from an unreadable file.
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Ok(false);
    }
    let mut header = [0u8; 50];
    match file.read_exact(&mut header) {
        Ok(()) => Ok(valid_header(&header, metadata.len())),
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => Ok(false),
        Err(error) => Err(error),
    }
}

// Format and modern macOS layout: Apple copyfile.c, apple_double_header_t.
// https://github.com/apple-oss-distributions/copyfile/blob/main/copyfile.c
// Validate the bounded directory of entries; never load resource forks/xattrs.
fn valid_header(header: &[u8; 50], file_len: u64) -> bool {
    let word = |offset| u32::from_be_bytes(header[offset..offset + 4].try_into().unwrap()) as u64;
    if word(0) != 0x0005_1607
        || word(4) != 0x0002_0000
        || u16::from_be_bytes([header[24], header[25]]) != 2
        || word(26) != 9 // Finder info, including extended attributes
        || word(38) != 2
    // Resource fork (possibly empty)
    {
        return false;
    }
    let finder_start = word(30);
    let finder_len = word(34);
    let resource_start = word(42);
    let resource_len = word(46);
    finder_start >= header.len() as u64
        && finder_len >= 32
        && finder_start + finder_len <= file_len
        && (resource_len == 0 && resource_start == 0
            || resource_start >= finder_start + finder_len
                && resource_start + resource_len <= file_len)
}

#[cfg(test)]
pub(crate) fn test_fixture() -> Vec<u8> {
    tests::fixture()
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn fixture() -> Vec<u8> {
        let mut bytes = vec![0u8; 82];
        for (offset, value) in [
            (0, 0x0005_1607u32),
            (4, 0x0002_0000),
            (26, 9),
            (30, 50),
            (34, 32),
            (38, 2),
            (42, 82),
        ] {
            bytes[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
        }
        bytes[24..26].copy_from_slice(&2u16.to_be_bytes());
        bytes
    }

    #[test]
    fn accepts_bounded_macos_header_and_optional_resource_fork() {
        let mut bytes = fixture();
        assert!(valid_header(bytes[..50].try_into().unwrap(), 82));
        bytes[46..50].copy_from_slice(&10u32.to_be_bytes());
        assert!(valid_header(bytes[..50].try_into().unwrap(), 92));
        assert!(!valid_header(bytes[..50].try_into().unwrap(), 91));
    }

    #[test]
    fn rejects_fake_truncated_overlapping_or_unsupported_headers() {
        let bytes = fixture();
        assert!(!valid_header(bytes[..50].try_into().unwrap(), 81));
        for (offset, value) in [
            (0, 0u32),
            (4, 0),
            (26, 1),
            (30, 49),
            (34, 31),
            (34, u32::MAX),
            (38, 9),
            (42, 60),
        ] {
            let mut changed = bytes.clone();
            changed[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
            assert!(!valid_header(changed[..50].try_into().unwrap(), 82));
        }
        let mut changed = bytes;
        changed[25] = 3;
        assert!(!valid_header(changed[..50].try_into().unwrap(), 82));
    }

    #[test]
    fn companion_names_cannot_point_outside_their_directory() {
        assert_eq!(
            companion_owner(Path::new("dir/._.hidden")),
            Some(PathBuf::from("dir/.hidden"))
        );
        assert_eq!(
            companion_owner(Path::new("dir/._Grüße")),
            Some(PathBuf::from("dir/Grüße"))
        );
        for name in ["._", "._.", "._..", "ordinary"] {
            assert!(companion_owner(Path::new(name)).is_none());
        }
    }
}
