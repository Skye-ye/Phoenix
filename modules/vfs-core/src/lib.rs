#![no_std]
#![no_main]

mod dentry;
mod file;
mod file_system_type;
mod inode;
mod super_block;
mod utils;

#[macro_use]
extern crate bitflags;
extern crate alloc;

pub const PERMISSION_LEN: usize = 9;

use core::sync::atomic::{AtomicUsize, Ordering};

pub use dentry::*;
pub use file::*;
pub use file_system_type::*;
pub use inode::*;
pub use super_block::*;
use sync::mutex::SpinNoIrqLock;
pub use utils::*;

type Mutex<T> = SpinNoIrqLock<T>;

static INODE_NUMBER: AtomicUsize = AtomicUsize::new(0);

fn alloc_ino() -> usize {
    INODE_NUMBER.fetch_add(1, Ordering::Relaxed)
}
