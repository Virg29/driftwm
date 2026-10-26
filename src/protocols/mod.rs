pub mod ext_workspace;
pub mod foreign_toplevel;
pub mod gamma_control;
pub mod image_capture_source;
pub mod image_copy_capture;
pub mod output_management;
pub mod output_power;
pub mod screencopy;
pub mod virtual_keyboard;

/// Whether a client-sent fd is one the compositor can read on the event loop:
/// a regular file or memfd, as the protocols that hand one over ask for. A
/// pipe or socket would block until the other end writes.
fn is_regular_file(file: &std::fs::File) -> bool {
    file.metadata().is_ok_and(|m| m.is_file())
}

#[cfg(test)]
mod tests {
    use super::is_regular_file;
    use std::fs::File;
    use std::os::fd::{FromRawFd, OwnedFd};

    #[test]
    fn regular_files_and_memfds_are_readable_on_the_loop() {
        let path = std::env::temp_dir().join(format!("driftwm-fd-test-{}", std::process::id()));
        let file = File::create(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert!(is_regular_file(&file));

        // SAFETY: a valid NUL-terminated name; the fd is checked before use.
        let memfd = unsafe { libc::memfd_create(c"driftwm-test".as_ptr(), libc::MFD_CLOEXEC) };
        assert!(memfd >= 0);
        // SAFETY: memfd_create returned a fresh fd this test owns.
        assert!(is_regular_file(&File::from(unsafe {
            OwnedFd::from_raw_fd(memfd)
        })));
    }

    #[test]
    fn pipes_and_sockets_are_not() {
        let mut fds = [0; 2];
        // SAFETY: pipe writes two fresh fds into the array.
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        // SAFETY: both fds are fresh and owned here from now on.
        let (read, _write) = unsafe { (File::from_raw_fd(fds[0]), File::from_raw_fd(fds[1])) };
        assert!(!is_regular_file(&read));

        let (socket, _peer) = std::os::unix::net::UnixStream::pair().unwrap();
        assert!(!is_regular_file(&File::from(OwnedFd::from(socket))));
    }
}
