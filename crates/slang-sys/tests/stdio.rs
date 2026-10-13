// SPDX-License-Identifier: Apache-2.0

//! The shim never writes to the process's stdout or stderr, and never
//! colors its text, even when stderr is a terminal. One test, since it
//! replaces file descriptors 1 and 2 for the whole process.

use std::io::Write as _;
use std::os::fd::RawFd;

use slang_sys::{AstJson, Request, Response, Stage, run};

fn elaborate(args: &[&str]) -> Response {
    run(&Request {
        args: args.iter().map(|a| a.to_string()).collect(),
        stage: Stage::Elaborate,
        ast_json: AstJson::None,
    })
    .unwrap()
}

/// Points `fd` at `to`; returns a copy of the old `fd` to restore.
fn redirect(fd: RawFd, to: RawFd) -> RawFd {
    // SAFETY: plain descriptor calls on descriptors this test owns.
    unsafe {
        let saved = libc::dup(fd);
        assert!(saved >= 0);
        assert!(libc::dup2(to, fd) >= 0);
        saved
    }
}

fn restore(fd: RawFd, saved: RawFd) {
    // SAFETY: as in `redirect`.
    unsafe {
        assert!(libc::dup2(saved, fd) >= 0);
        libc::close(saved);
    }
}

/// A pipe as `(read, write)`.
fn pipe() -> (RawFd, RawFd) {
    let mut fds = [0; 2];
    // SAFETY: `pipe` fills the array.
    assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
    (fds[0], fds[1])
}

/// Everything in the pipe after its write end is closed.
fn drain(read: RawFd) -> Vec<u8> {
    let mut out = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        // SAFETY: reads into a buffer of the given size.
        let n = unsafe { libc::read(read, buf.as_mut_ptr().cast(), buf.len()) };
        if n <= 0 {
            break;
        }
        out.extend_from_slice(&buf[..n as usize]);
    }
    // SAFETY: closes a descriptor this test owns.
    unsafe { libc::close(read) };
    out
}

#[test]
fn shim_never_prints_and_never_colors() {
    let mut bad = tempfile::Builder::new().suffix(".sv").tempfile().unwrap();
    bad.write_all(b"module m(;\nendmodule\n").unwrap();
    let bad = bad.path().to_str().unwrap().to_string();

    let (out_read, out_write) = pipe();
    let (err_read, err_write) = pipe();
    let saved_out = redirect(1, out_write);
    let saved_err = redirect(2, err_write);
    let missing = elaborate(&["/nonexistent/x.sv", "-I", "/nonexistent/dir"]);
    let parse_error = elaborate(&[&bad]);
    restore(1, saved_out);
    restore(2, saved_err);
    // SAFETY: closes the write ends this test owns.
    unsafe {
        libc::close(out_write);
        libc::close(err_write);
    }
    assert!(!missing.ok && !parse_error.ok);
    assert_eq!(String::from_utf8_lossy(&drain(out_read)), "");
    assert_eq!(String::from_utf8_lossy(&drain(err_read)), "");

    let (mut master, mut slave) = (0, 0);
    // SAFETY: `openpty` fills both descriptors.
    let opened = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    assert_eq!(opened, 0);
    let saved_err = redirect(2, slave);
    let on_tty = elaborate(&[&bad]);
    restore(2, saved_err);
    // SAFETY: closes the pty descriptors this test owns.
    unsafe {
        libc::close(slave);
        libc::close(master);
    }
    assert!(!on_tty.rendered.is_empty());
    assert!(!on_tty.rendered.contains("\x1b["), "{:?}", on_tty.rendered);
}
