//! Work that happens somewhere other than the interface.
//!
//! A frame has about sixteen milliseconds. Anything that might take longer than
//! that, reading a file, talking to a network, laying out a large document, has
//! to happen off the thread that draws, or the window stops responding while it
//! runs.
//!
//! [`spawn`] starts a closure on a pool of worker threads and hands back a
//! [`Task`], which the interface asks for a result whenever it draws:
//!
//! ```ignore
//! // In a click handler.
//! app.search = Some(spawn(move || read_the_index(&query)));
//!
//! // In the root, next frame and every frame after.
//! if let Some(results) = app.search.as_ref().and_then(Task::take) {
//!     app.results = results;
//! }
//! ```
//!
//! Polling rather than a callback, because a callback would arrive on a worker
//! thread and everything an interface owns, the element state, the [`Model`]s,
//! the text caches, lives on the one that draws. A result that has to cross
//! threads is easier to reason about when the crossing is a single place the
//! application chose.
//!
//! Finishing wakes the event loop, so the frame that reads the result happens
//! on its own. Nothing has to poll on a timer.
//!
//! [`Model`]: crate::model::Model

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};

type Job = Box<dyn FnOnce() + Send + 'static>;

/// Asks the platform for another turn of the event loop.
///
/// Set once by the runtime when an application starts. A process with no window
/// running, a test or a command line tool, leaves it unset and tasks still run;
/// there is simply nothing to wake.
static WAKE: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();

/// Tell the task system how to wake the interface.
pub(crate) fn set_waker(wake: impl Fn() + Send + Sync + 'static) {
    // Ignoring a second call on purpose: only one event loop can exist, and a
    // second application in the same process is a misuse rather than a reason
    // to panic in a library.
    let _ = WAKE.set(Box::new(wake));
}

fn wake() {
    if let Some(wake) = WAKE.get() {
        wake();
    }
}

/// A handle to work running off the interface's thread.
///
/// Cloning shares the same work rather than starting it again, so a handle can
/// be kept in one place and read from another.
pub struct Task<T> {
    inner: Arc<Shared<T>>,
}

struct Shared<T> {
    result: Mutex<Option<T>>,
    finished: AtomicBool,
    cancelled: AtomicBool,
}

impl<T> Clone for Task<T> {
    fn clone(&self) -> Self {
        Task {
            inner: self.inner.clone(),
        }
    }
}

impl<T> std::fmt::Debug for Task<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Task")
            .field("finished", &self.is_finished())
            .field("cancelled", &self.is_cancelled())
            .finish()
    }
}

impl<T> Task<T> {
    /// Whether the work has run to completion.
    ///
    /// True even after the result has been taken, so this answers "did it
    /// finish" rather than "is there something to collect".
    #[inline]
    pub fn is_finished(&self) -> bool {
        self.inner.finished.load(Ordering::Acquire)
    }

    /// Whether the work was asked to stop.
    #[inline]
    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::Relaxed)
    }

    /// Take the result, if there is one waiting.
    ///
    /// Returns `Some` exactly once. A frame that reads it and a frame that runs
    /// before it finished both behave the same way: they get `None` and draw
    /// whatever they already had.
    pub fn take(&self) -> Option<T> {
        if !self.is_finished() {
            return None;
        }
        self.inner.result.lock().ok()?.take()
    }

    /// Whether a result is waiting to be taken.
    pub fn is_ready(&self) -> bool {
        self.is_finished()
            && self
                .inner
                .result
                .lock()
                .map(|result| result.is_some())
                .unwrap_or(false)
    }

    /// Ask the work to stop.
    ///
    /// Cooperative, and deliberately so: a thread cannot be safely killed from
    /// outside while it holds a lock or a file. Work that can be interrupted
    /// checks [`is_cancelled`](Self::is_cancelled) as it goes; work that
    /// cannot runs to the end and has its result dropped.
    pub fn cancel(&self) {
        self.inner.cancelled.store(true, Ordering::Relaxed);
    }
}

/// Run a closure on a worker thread.
///
/// The interface is woken when it finishes, so the next frame can collect the
/// result with [`Task::take`].
///
/// The closure and its result both have to cross a thread, so both are `Send`.
/// That rules out passing a [`Model`](crate::model::Model) in, which is the
/// point: interface state belongs to the thread that draws, and a task returns
/// a value for that thread to apply rather than reaching over and applying it.
pub fn spawn<T, F>(work: F) -> Task<T>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let inner = Arc::new(Shared {
        result: Mutex::new(None),
        finished: AtomicBool::new(false),
        cancelled: AtomicBool::new(false),
    });

    let task = Task {
        inner: inner.clone(),
    };
    let handle = Task { inner };

    pool().submit(Box::new(move || {
        // A task cancelled before it started never runs at all.
        if handle.is_cancelled() {
            handle.inner.finished.store(true, Ordering::Release);
            wake();
            return;
        }

        let value = work();

        if !handle.is_cancelled() {
            if let Ok(mut slot) = handle.inner.result.lock() {
                *slot = Some(value);
            }
        }
        // Released after the result is in place, so anything that sees the
        // flag also sees the value.
        handle.inner.finished.store(true, Ordering::Release);
        wake();
    }));

    task
}

/// A task that is already finished, holding `value`.
///
/// Useful where a value is sometimes at hand and sometimes has to be fetched,
/// so both paths can be the same type.
pub fn ready<T: Send + 'static>(value: T) -> Task<T> {
    Task {
        inner: Arc::new(Shared {
            result: Mutex::new(Some(value)),
            finished: AtomicBool::new(true),
            cancelled: AtomicBool::new(false),
        }),
    }
}

// ---------------------------------------------------------------------------
// The pool
// ---------------------------------------------------------------------------

/// How many workers there are.
///
/// Interface work is mostly waiting on something, a file or a socket, rather
/// than computing, so this is not the number of cores. It is small enough that
/// an application which never spawns anything pays for nothing, and the pool
/// is only built the first time something is spawned.
fn worker_count() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(2, 8)
}

struct Pool {
    queue: Mutex<VecDeque<Job>>,
    work_arrived: Condvar,
}

impl Pool {
    fn submit(&self, job: Job) {
        if let Ok(mut queue) = self.queue.lock() {
            queue.push_back(job);
            self.work_arrived.notify_one();
        }
    }
}

static POOL: OnceLock<&'static Pool> = OnceLock::new();

fn pool() -> &'static Pool {
    POOL.get_or_init(|| {
        // Leaked on purpose. The workers outlive every caller and run until the
        // process ends, so there is nothing to give the memory back to, and a
        // static reference is what lets them park on the queue forever.
        let pool: &'static Pool = Box::leak(Box::new(Pool {
            queue: Mutex::new(VecDeque::new()),
            work_arrived: Condvar::new(),
        }));

        for index in 0..worker_count() {
            let started = std::thread::Builder::new()
                .name(format!("guirs worker {index}"))
                .spawn(move || worker(pool));
            if let Err(error) = started {
                log::warn!("could not start a task worker: {error}");
            }
        }
        pool
    })
}

fn worker(pool: &'static Pool) {
    loop {
        // A worker panicked while holding the queue. Nothing here can put that
        // right, and spinning on a poisoned lock would burn a core, so this
        // worker retires and the others carry on.
        let Ok(mut queue) = pool.queue.lock() else {
            return;
        };

        let job = loop {
            if let Some(job) = queue.pop_front() {
                break job;
            }
            match pool.work_arrived.wait(queue) {
                Ok(woken) => queue = woken,
                Err(_) => return,
            }
        };

        // Dropped before running the job, so one long task does not stop every
        // other worker from picking up its own.
        drop(queue);
        job();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;
    use std::time::Duration;

    /// Wait for a task, with a ceiling so a broken pool fails rather than hangs.
    fn settle<T>(task: &Task<T>) -> Option<T> {
        for _ in 0..2_000 {
            if let Some(value) = task.take() {
                return Some(value);
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        None
    }

    #[test]
    fn a_task_hands_back_what_it_computed() {
        let task = spawn(|| 2 + 2);
        assert_eq!(settle(&task), Some(4));
    }

    #[test]
    fn a_result_is_taken_exactly_once() {
        let task = spawn(|| String::from("once"));
        assert_eq!(settle(&task).as_deref(), Some("once"));
        // Finished, but there is nothing left to collect.
        assert!(task.is_finished());
        assert!(task.take().is_none());
        assert!(!task.is_ready());
    }

    #[test]
    fn nothing_is_ready_before_the_work_is_done() {
        let (release, blocked) = channel::<()>();
        let task = spawn(move || {
            let _ = blocked.recv();
            7
        });

        assert!(!task.is_finished());
        assert!(task.take().is_none());

        let _ = release.send(());
        assert_eq!(settle(&task), Some(7));
    }

    #[test]
    fn a_handle_can_be_cloned_and_read_from_either() {
        let task = spawn(|| 41);
        let other = task.clone();
        // Whichever is asked first gets it, and it is the same piece of work.
        let value = settle(&other);
        assert_eq!(value, Some(41));
        assert!(task.is_finished());
        assert!(task.take().is_none());
    }

    #[test]
    fn a_cancelled_task_delivers_nothing_even_if_it_finished() {
        // Held inside the closure so the cancellation lands while the work is
        // definitely still running, rather than racing it.
        let (release, blocked) = channel::<()>();
        let (started, running) = channel::<()>();
        let task = spawn(move || {
            let _ = started.send(());
            let _ = blocked.recv();
            "should not be collected"
        });

        running
            .recv_timeout(Duration::from_secs(5))
            .expect("the task never started");
        task.cancel();
        assert!(task.is_cancelled());
        let _ = release.send(());

        for _ in 0..5_000 {
            if task.is_finished() {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        // It ran to the end, because a thread cannot be stopped from outside.
        // What it must not do is hand the value over.
        assert!(task.is_finished());
        assert!(task.take().is_none(), "a cancelled task delivered a result");
    }

    #[test]
    fn work_can_notice_that_it_was_cancelled() {
        let task: Task<usize> = {
            let inner = Arc::new(Shared {
                result: Mutex::new(None),
                finished: AtomicBool::new(false),
                cancelled: AtomicBool::new(false),
            });
            let handle = Task {
                inner: inner.clone(),
            };
            let watched = Task {
                inner: inner.clone(),
            };
            pool().submit(Box::new(move || {
                let mut spins = 0usize;
                while !watched.is_cancelled() && spins < 100_000 {
                    spins += 1;
                    std::thread::sleep(Duration::from_micros(50));
                }
                if let Ok(mut slot) = watched.inner.result.lock() {
                    *slot = Some(spins);
                }
                watched.inner.finished.store(true, Ordering::Release);
            }));
            let _ = handle;
            Task { inner }
        };

        std::thread::sleep(Duration::from_millis(5));
        task.cancel();
        let spins = settle(&task).expect("the work never stopped");
        assert!(spins < 100_000, "the work ignored the cancellation");
    }

    #[test]
    fn many_tasks_all_finish() {
        let tasks: Vec<Task<usize>> = (0..64).map(|n| spawn(move || n * 2)).collect();
        let mut total = 0usize;
        for task in &tasks {
            total += settle(task).expect("a task never finished");
        }
        assert_eq!(total, (0..64).map(|n: usize| n * 2).sum::<usize>());
    }

    #[test]
    fn a_ready_task_needs_no_worker() {
        let task = ready(9);
        assert!(task.is_finished());
        assert!(task.is_ready());
        assert_eq!(task.take(), Some(9));
        assert!(task.take().is_none());
    }

    #[test]
    fn the_pool_is_a_sensible_size() {
        // Small on purpose: interface work waits more than it computes, and
        // one thread per core would reserve stacks for nothing.
        let count = worker_count();
        assert!((2..=8).contains(&count), "the pool has {count} workers");
    }
}
