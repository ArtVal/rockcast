//! Small bounded runtime for blocking application jobs.
//!
//! RockCast uses blocking audio and Cast APIs. A fixed worker set keeps those
//! operations off the egui thread without creating an unbounded OS thread for
//! every click, delayed tap, or channel forwarder.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
};

use parking_lot::Mutex;

type Job = Box<dyn FnOnce(CancelToken) + Send + 'static>;

enum Command {
    Run(Job),
    Shutdown,
}

#[derive(Clone)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

pub struct BackgroundRuntime {
    tx: mpsc::Sender<Command>,
    cancelled: Arc<AtomicBool>,
    workers: Vec<thread::JoinHandle<()>>,
}

impl BackgroundRuntime {
    pub fn new(worker_count: usize) -> Self {
        let worker_count = worker_count.max(2);
        let (tx, rx) = mpsc::channel::<Command>();
        let rx = Arc::new(Mutex::new(rx));
        let cancelled = Arc::new(AtomicBool::new(false));
        let mut workers = Vec::with_capacity(worker_count);

        for index in 0..worker_count {
            let rx = Arc::clone(&rx);
            let cancelled = Arc::clone(&cancelled);
            workers.push(
                thread::Builder::new()
                    .name(format!("rockcast-bg-{index}"))
                    .spawn(move || {
                        loop {
                            let command = rx.lock().recv();
                            match command {
                                Ok(Command::Run(job)) => {
                                    if !cancelled.load(Ordering::Acquire) {
                                        job(CancelToken(Arc::clone(&cancelled)));
                                    }
                                }
                                Ok(Command::Shutdown) | Err(_) => break,
                            }
                        }
                    })
                    .expect("spawn RockCast background worker"),
            );
        }

        Self {
            tx,
            cancelled,
            workers,
        }
    }

    pub fn spawn(
        &self,
        job: impl FnOnce(CancelToken) + Send + 'static,
    ) -> Result<(), &'static str> {
        if self.cancelled.load(Ordering::Acquire) {
            return Err("runtime is shutting down");
        }
        self.tx
            .send(Command::Run(Box::new(job)))
            .map_err(|_| "runtime workers stopped")
    }

    pub fn cancel_token(&self) -> CancelToken {
        CancelToken(Arc::clone(&self.cancelled))
    }

    pub fn shutdown(&mut self) {
        if self.cancelled.swap(true, Ordering::AcqRel) {
            return;
        }
        for _ in 0..self.workers.len() {
            let _ = self.tx.send(Command::Shutdown);
        }
        // Blocking HTTP implementations may still be inside an OS read. Do not
        // join here: window close must remain bounded. Workers observe the token
        // before accepting another job and the process exit remains the final
        // safety boundary on Windows.
        self.workers.clear();
    }
}

impl Drop for BackgroundRuntime {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn bounded_runtime_runs_jobs_and_cancels() {
        let mut runtime = BackgroundRuntime::new(2);
        let (tx, rx) = mpsc::channel();
        runtime.spawn(move |_| tx.send(7).unwrap()).unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_secs(1)).unwrap(), 7);
        let token = runtime.cancel_token();
        runtime.shutdown();
        assert!(token.is_cancelled());
        assert!(runtime.spawn(|_| {}).is_err());
    }
}
