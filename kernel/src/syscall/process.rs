//! Syscall for processes operations.

use alloc::{
    string::{String, ToString},
    vec::Vec,
};

use async_utils::yield_now;
use memory::VirtAddr;
use signal::{
    siginfo::*,
    sigset::{Sig, SigSet},
};
use systype::{SysError, SysResult, SyscallResult};
use vfs::{sys_root_dentry, DISK_FS_NAME, FS_MANAGER};
use vfs_core::{InodeMode, OpenFlags, AT_FDCWD};

use crate::{
    mm::{UserReadPtr, UserWritePtr},
    processor::hart::current_task,
    syscall::{at_helper, resolve_path},
    task::{signal::WaitExpectSignal, spawn_user_task, PGid, Pid, TASK_MANAGER},
};

bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    /// See in "bits/sched.h"
    pub struct CloneFlags: u64 {
        /// Set if VM shared between processes.
        const VM = 0x0000100;
        /// Set if fs info shared between processes.
        const FS = 0x0000200;
        /// Set if open files shared between processes.
        const FILES = 0x0000400;
        /// Set if signal handlers shared.
        const SIGHAND = 0x00000800;
        /// Set if we want to have the same parent as the cloner.
        const PARENT = 0x00008000;
        /// Set to add to same thread group.
        const THREAD = 0x00010000;
        /// Set TLS info.
        const SETTLS = 0x00080000;
        /// Store TID in userlevel buffer before MM copy.
        const PARENT_SETTID = 0x00100000;
        /// Register exit futex and memory location to clear.
        const CHILD_CLEARTID = 0x00200000;
        /// Store TID in userlevel buffer in the child.
        const CHILD_SETTID = 0x01000000;
    }
}

bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    /// See in "bits/waitflags.h"
    pub struct WaitOptions: i32 {
        /// Don't block waiting.
        const WNOHANG = 0x00000001;
        /// Report status of stopped children.
        const WUNTRACED = 0x00000002;
        /// Report continued child.
        const WCONTINUED = 0x00000008;
    }
}

/// _exit() system call terminates only the calling thread, and actions such as
/// reparenting child processes or sending SIGCHLD to the parent process are
/// performed only if this is the last thread in the thread group.
pub fn sys_exit(exit_code: i32) -> SyscallResult {
    let task = current_task();
    task.set_zombie();
    // non-leader thread are detached (see CLONE_THREAD flag in manual page clone.2)
    if task.is_leader() {
        task.set_exit_code((exit_code & 0xFF) << 8);
    }
    Ok(0)
}

/// This system call terminates all threads in the calling process's thread
/// group.
pub fn sys_exit_group(exit_code: i32) -> SyscallResult {
    let task = current_task();
    task.with_thread_group(|tg| {
        for t in tg.iter() {
            t.set_zombie();
        }
    });
    task.set_exit_code((exit_code & 0xFF) << 8);
    Ok(0)
}

pub fn sys_gettid() -> SyscallResult {
    Ok(current_task().tid())
}

/// getpid() returns the process ID (PID) of the calling process.
pub fn sys_getpid() -> SyscallResult {
    Ok(current_task().pid())
}

/// getppid() returns the process ID of the parent of the calling process. This
/// will be either the ID of the process that created this process using fork(),
/// or, if that process has already terminated, the ID of the process to which
/// this process has been reparented.
pub fn sys_getppid() -> SyscallResult {
    Ok(current_task().ppid())
}

/// NOTE: A thread can, and by default will, wait on children of other threads
/// in the same thread group.
// TODO: More options and process group support.
// PERF: use event bus to notify this task when child exits
pub async fn sys_wait4(
    pid: i32,
    wstatus: UserWritePtr<i32>,
    option: i32,
    _rusage: usize,
) -> SyscallResult {
    let task = current_task();
    let option = WaitOptions::from_bits_truncate(option);
    #[derive(Debug)]
    enum WaitFor {
        // wait for any child process in the specific process group
        PGid(PGid),
        // wait for any child process
        AnyChild,
        // wait for any child process in the same process group of the calling process
        AnyChildInGroup,
        // wait for the child process with the specific pid
        Pid(Pid),
    }
    let target = match pid {
        -1 => WaitFor::AnyChild,
        0 => WaitFor::AnyChildInGroup,
        p if p > 0 => WaitFor::Pid(p as Pid),
        p => WaitFor::PGid(p as PGid),
    };
    log::info!("[sys_wait4] target: {target:?}, option: {option:?}");
    // 首先检查一遍等待的进程是否已经是zombie了
    let children = task.children();
    if children.is_empty() {
        log::warn!("[sys_wait4] fail: no child");
        return Err(SysError::ECHILD);
    }
    let res_task = match target {
        WaitFor::AnyChild => children.values().find(|c| c.is_zombie()),
        WaitFor::Pid(pid) => {
            if let Some(child) = children.get(&pid) {
                if child.is_zombie() {
                    Some(child)
                } else {
                    None
                }
            } else {
                log::warn!("[sys_wait4] fail: no child with pid {pid}");
                return Err(SysError::ECHILD);
            }
        }
        WaitFor::PGid(_) => unimplemented!(),
        WaitFor::AnyChildInGroup => unimplemented!(),
    };
    if let Some(res_task) = res_task {
        task.time_stat()
            .update_child_time(res_task.time_stat().user_system_time());
        if wstatus.not_null() {
            // wstatus stores signal in the lowest 8 bits and exit code in higher 8 bits
            // wstatus macros can be found in "bits/waitstatus.h"
            let exit_code = res_task.exit_code();
            log::debug!("[sys_wait4] wstatus: {exit_code:#x}");
            wstatus.write(&task, exit_code)?;
        }
        let tid = res_task.tid();
        task.remove_child(tid);
        TASK_MANAGER.remove(tid);
        return Ok(tid);
    } else if option.contains(WaitOptions::WNOHANG) {
        return Ok(0);
    } else {
        // 如果等待的进程还不是zombie，那么本进程进行await，
        // 直到等待的进程do_exit然后发送SIGCHLD信号唤醒自己
        let (child_pid, exit_code, utime, stime) = match target {
            WaitFor::AnyChild => {
                let si = WaitExpectSignal::new(&task, Sig::SIGCHLD).await;
                match si.details {
                    SigDetails::CHLD {
                        pid,
                        status,
                        utime,
                        stime,
                    } => (pid, status, utime, stime),
                    _ => unreachable!(),
                }
            }
            WaitFor::Pid(pid) => loop {
                let si = WaitExpectSignal::new(&task, Sig::SIGCHLD).await;
                match si.details {
                    SigDetails::CHLD {
                        pid: child_pid,
                        status,
                        utime,
                        stime,
                    } => {
                        if child_pid == pid {
                            break (pid, status, utime, stime);
                        }
                    }
                    _ => unreachable!(),
                }
            },
            WaitFor::AnyChildInGroup => unimplemented!(),
            WaitFor::PGid(_) => unimplemented!(),
        };
        task.time_stat().update_child_time((utime, stime));
        if wstatus.not_null() {
            // wstatus stores signal in the lowest 8 bits and exit code in higher 8 bits
            // wstatus macros can be found in <bits/waitstatus.h>
            log::trace!("[sys_wait4] wstatus: {:#x}", exit_code);
            wstatus.write(&task, exit_code)?;
        }
        task.remove_child(child_pid);
        TASK_MANAGER.remove(child_pid);
        return Ok(child_pid);
    }
}

/// execve() executes the program referred to by pathname. This causes the
/// program that is currently being run by the calling process to be replaced
/// with a new program, with newly initialized stack, heap, and (initialized and
/// uninitialized) data segments.
///
/// If any of the threads in a thread group performs an execve(2), then all
/// threads other than the thread group leader are terminated, and the new
/// program is executed in the thread group leader.
pub async fn sys_execve(
    path: UserReadPtr<u8>,
    argv: UserReadPtr<usize>,
    envp: UserReadPtr<usize>,
) -> SyscallResult {
    let task = current_task();
    let mut path = path.read_cstr(&task)?;

    let read_2d_cstr = |ptr2d: UserReadPtr<usize>| -> SysResult<Vec<String>> {
        let ptr_vec: Vec<UserReadPtr<u8>> = ptr2d
            .read_cvec(&task)?
            .into_iter()
            .map(UserReadPtr::from)
            .collect();
        let mut result = Vec::new();
        for ptr in ptr_vec {
            let str = ptr.read_cstr(&task)?;
            result.push(str);
        }
        Ok(result)
    };

    let mut argv = read_2d_cstr(argv)?;
    let envp = read_2d_cstr(envp)?;

    log::info!("[sys_execve]: path: {path:?}, argv: {argv:?}, envp: {envp:?}",);

    // TODO: should we add envp

    if path.ends_with(".sh") {
        path = "/busybox".to_string();
        argv.insert(0, "busybox".to_string());
        argv.insert(1, "sh".to_string());
    }

    let file = resolve_path(&path)?.open()?;
    let elf_data = file.read_all().await?;
    task.do_execve(&elf_data, argv, envp);
    Ok(0)
}

// TODO:
pub fn sys_clone(
    flags: usize,
    stack: usize,
    _parent_tid_ptr: usize,
    _tls_ptr: usize,
    chilren_tid_ptr: usize,
) -> SyscallResult {
    let exit_signal = flags & 0xff;
    let flags = CloneFlags::from_bits(flags as u64 & !0xff).ok_or(SysError::EINVAL)?;
    log::info!("[sys_clone] flags {flags:?}");
    // if flags.contains(CloneFlags::THREAD) {
    // // EINVAL:
    // // CLONE_SIGHAND was specified in the flags mask, but CLONE_VM was not.
    // // CLONE_THREAD was specified in the flags mask, but CLONE_SIGHAND was not.
    // if !flags.contains(CloneFlags::SIGHAND) || !flags.contains(CloneFlags::VM) {
    //     return Err(SysError::EINVAL);
    // }
    // }
    let stack = if stack != 0 { Some(stack.into()) } else { None };
    let new_task = current_task().do_clone(flags, stack, chilren_tid_ptr);
    new_task.trap_context_mut().set_user_a0(0);
    let new_tid = new_task.tid();
    log::info!("[sys_clone] clone a new thread, tid {new_tid}, clone flags {flags:?}",);
    spawn_user_task(new_task);
    Ok(new_tid)
}

pub async fn sys_sched_yield() -> SyscallResult {
    yield_now().await;
    Ok(0)
}

/// The system call set_tid_address() sets the clear_child_tid value for the
/// calling thread to tidptr.
///
/// When a thread whose clear_child_tid is not NULL terminates, then, if the
/// thread is sharing memory with other threads, then 0 is written at the
/// address specified in clear_child_tid and the kernel performs the following
/// operation:
///
/// futex(clear_child_tid, FUTEX_WAKE, 1, NULL, NULL, 0);
///
/// The effect of this operation is to wake a single thread that is performing a
/// futex wait on the memory location. Errors from the futex wake operation are
/// ignored.
///
/// set_tid_address() always returns the caller's thread ID.
// TODO: do the futex wake up at the address when task terminates
pub fn sys_set_tid_address(tidptr: usize) -> SyscallResult {
    let task = current_task();
    task.tid_address_mut().clear_child_tid = Some(tidptr);
    Ok(task.tid())
}

/// getpgid() returns the PGID of the process specified by pid. If pid is zero,
/// the process ID of the calling process is used. (Retrieving the PGID of a
/// process other than the caller is rarely necessary, and the POSIX.1 getpgrp()
/// is preferred for that task.)
pub fn sys_getpgid(pid: usize) -> SyscallResult {
    let target_task = if pid == 0 {
        current_task()
    } else {
        TASK_MANAGER.get(pid).ok_or(SysError::ESRCH)?
    };

    Ok(target_task.pid().into())
}

/// setpgid() sets the PGID of the process specified by pid to pgid. If pid is
/// zero, then the process ID of the calling process is used. If pgid is zero,
/// then the PGID of the process specified by pid is made the same as its
/// process ID. If setpgid() is used to move a process from one process group to
/// another (as is done by some shells when creating pipelines), both process
/// groups must be part of the same session (see setsid(2) and credentials(7)).
/// In this case, the pgid specifies an existing process group to be joined and
/// the session ID of that group must match the session ID of the joining
/// process.
pub fn sys_setpgid(pid: usize, pgid: usize) -> SyscallResult {
    let target_task = if pid == 0 {
        current_task()
    } else {
        TASK_MANAGER.get(pid).ok_or(SysError::ESRCH)?
    };

    Ok(target_task.pid().into())
}

// TODO:
pub fn sys_getuid() -> SyscallResult {
    Ok(0)
}

// TODO:
pub fn sys_geteuid() -> SyscallResult {
    Ok(0)
}
