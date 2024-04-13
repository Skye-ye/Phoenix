#![no_std]
#![no_main]

extern crate user_lib;

use user_lib::{execve, fork, println, wait, yield_};

#[no_mangle]
fn main() -> i32 {
    if fork() == 0 {
        execve(
            "hello_world\0",
            &[
                "busybox\0".as_ptr(),
                "sh\0".as_ptr(),
                core::ptr::null::<u8>(),
            ],
            &[
                "PATH=/:/bin:/sbin:/usr/bin:/usr/local/bin:/usr/local/sbin:\0".as_ptr(),
                "LD_LIBRARY_PATH=/:/lib:/lib64/lp64d:/usr/lib:/usr/local/lib:\0".as_ptr(),
                "TERM=screen\0".as_ptr(),
                core::ptr::null::<u8>(),
            ],
        );
    } else {
        let mut exit_code: i32 = 0;
        let _pid = wait(&mut exit_code);
        yield_();
    }
    0
}
