use std::fs::File;
use std::os::fd::BorrowedFd;

/// The caller must keep its platform File open until this function returns.
/// Rust then owns an independent descriptor, including on cancelled requests.
pub fn duplicate_file(fd: i32) -> std::io::Result<File> {
    if fd < 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "invalid file descriptor",
        ));
    }
    // SAFETY: the native file-access plugin holds the File open throughout the
    // synchronous duplicate-file event. This function never owns that handle.
    let borrowed = unsafe { BorrowedFd::borrow_raw(fd) };
    Ok(File::from(borrowed.try_clone_to_owned()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Seek, SeekFrom, Write};
    use std::os::fd::AsRawFd;

    #[test]
    fn rust_descriptor_survives_closing_the_platform_file() {
        let mut original = tempfile::tempfile().unwrap();
        original.write_all(b"selected content").unwrap();
        original.seek(SeekFrom::Start(0)).unwrap();
        let mut owned = duplicate_file(original.as_raw_fd()).unwrap();
        drop(original);
        let mut text = String::new();
        owned.read_to_string(&mut text).unwrap();
        assert_eq!(text, "selected content");
    }

    #[test]
    fn negative_descriptor_is_rejected() {
        assert_eq!(
            duplicate_file(-1).unwrap_err().kind(),
            std::io::ErrorKind::InvalidInput
        );
    }
}
