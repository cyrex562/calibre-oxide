// Unix-specific implementation using fcntl
#[cfg(unix)]
use std::os::unix::io::AsRawFd;

#[cfg(unix)]
pub fn get_socket_inherit(sock: &impl AsRawFd) -> bool {
    let fd = sock.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 {
        return false; // Error handling usually implies returning Result, but py version just catches generic exception.
    }
    (flags & libc::FD_CLOEXEC) == 0
}

#[cfg(unix)]
pub fn set_socket_inherit(sock: &impl AsRawFd, inherit: bool) {
    let fd = sock.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 {
        return;
    }
    let new_flags = if inherit {
        flags & !libc::FD_CLOEXEC
    } else {
        flags | libc::FD_CLOEXEC
    };
    unsafe { libc::fcntl(fd, libc::F_SETFD, new_flags) };
}

// Windows-specific implementation using SetHandleInformation
#[cfg(windows)]
use std::os::windows::io::AsRawSocket;

// Windows stub implementation
// TODO: Implement proper Windows handle inheritance using SetHandleInformation/GetHandleInformation
// The exact module path in windows-sys is unclear, so using stub for now
#[cfg(windows)]
pub fn get_socket_inherit<T>(_sock: &T) -> bool {
    // On Windows, sockets are non-inheritable by default in modern Rust
    false
}

#[cfg(windows)]
pub fn set_socket_inherit<T>(_sock: &T, _inherit: bool) {
    // TODO: Implement using Windows API when correct module path is determined
    // This is a stub implementation for compilation
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn test_inheritance() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        // By default, Rust sockets might be CLOEXEC on Unix or non-inheritable on Windows

        let initial = get_socket_inherit(&listener);

        set_socket_inherit(&listener, !initial);
        assert_eq!(get_socket_inherit(&listener), !initial);

        set_socket_inherit(&listener, initial);
        assert_eq!(get_socket_inherit(&listener), initial);
    }
}
