#![no_std]
#![no_main]

use alloc::collections::VecDeque;
use core::future::Future;

use async_task::{Runnable, ScheduleInfo, Task, WithInfo};
use sync::mutex::SpinNoIrqLock;

static TASK_QUEUE: TaskQueue = TaskQueue::new();

struct TaskQueue {
    queue: SpinNoIrqLock<VecDeque<Runnable>>,
}

impl TaskQueue {
    pub const fn new() -> Self {
        Self {
            queue: SpinNoIrqLock::new(VecDeque::new()),
        }
    }
    pub fn push(&self, runnable: Runnable) {
        self.queue.lock().push_back(runnable);
    }
    pub fn push_preempt(&self, runnable: Runnable) {
        self.queue.lock().push_front(runnable);
    }
    pub fn fetch(&self) -> Option<Runnable> {
        self.queue.lock().pop_front()
    }
}

/// Add a task into task queue
pub fn spawn<F>(future: F) -> (Runnable, Task<F::Output>)
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    let schedule = move |runnable: Runnable, info: ScheduleInfo| {
        if info.woken_while_running {
            // i.e `yield_now()`
            TASK_QUEUE.push(runnable);
        } else {
            // i.e. woken up by some signal
            TASK_QUEUE.push_preempt(runnable);
        }
    };
    async_task::spawn(future, WithInfo(schedule))
}

/// Return the number of the tasks executed
pub fn run_until_idle() -> usize {
    let mut n = 0;
    while let Some(task) = TASK_QUEUE.fetch() {
        task.run();
        n += 1;
    }
    n
}
