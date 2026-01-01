use std::os::unix::io::AsRawFd;

pub fn get_socket_inherit(sock: &impl AsRawFd) -> bool {
    let fd = sock.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 {
        return false; // Error handling usually implies returning Result, but py version just catches generic exception.
    }
    (flags & libc::FD_CLOEXEC) == 0
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn test_inheritance() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        // By default, Rust sockets might be CLOEXEC?
        // socket2 sets CLOEXEC by default on new sockets.
        // Let's check.
        // Actually, std::net sockets are usually CLOEXEC on Linux now in newer Rust?
        
        let initial = get_socket_inherit(&listener);
        
        set_socket_inherit(&listener, !initial);
        assert_eq!(get_socket_inherit(&listener), !initial);
        
        set_socket_inherit(&listener, initial);
        assert_eq!(get_socket_inherit(&listener), initial);
    }
}
