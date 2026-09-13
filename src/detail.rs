use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};

use crate::app::{DetailEffect, DetailResult};
use crate::event::DetailRequester;
use crate::github::{CancellationToken, CommandRunner, GitHubLoader};

struct DetailWorker {
    cancellation: CancellationToken,
    handle: JoinHandle<()>,
}

/// Owns the short-lived workers used for explicit commit-detail requests.
/// Finished workers are reaped without blocking the terminal event loop; all
/// remaining workers are cancelled and joined during shutdown.
pub struct DetailSession {
    sender: Sender<DetailResult>,
    receiver: Receiver<DetailResult>,
    workers: HashMap<u64, DetailWorker>,
}

impl DetailSession {
    pub fn new() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            sender,
            receiver,
            workers: HashMap::new(),
        }
    }

    fn reap_finished(&mut self) {
        let finished: Vec<_> = self
            .workers
            .iter()
            .filter_map(|(request_id, worker)| worker.handle.is_finished().then_some(*request_id))
            .collect();
        for request_id in finished {
            if let Some(worker) = self.workers.remove(&request_id) {
                let _ = worker.handle.join();
            }
        }
    }
}

impl Default for DetailSession {
    fn default() -> Self {
        Self::new()
    }
}

impl DetailRequester for DetailSession {
    fn request(&mut self, effect: DetailEffect) {
        let DetailEffect::Request {
            request_id,
            key,
            repository,
        } = effect
        else {
            return;
        };
        self.reap_finished();
        if self.workers.contains_key(&request_id) {
            return;
        }

        let cancellation = CancellationToken::default();
        let worker_cancellation = cancellation.clone();
        let sender = self.sender.clone();
        let handle = thread::spawn(move || {
            let loader = GitHubLoader::new(CommandRunner);
            let outcome = loader.load_commit_detail(&repository, &key.sha, &worker_cancellation);
            let _ = sender.send(DetailResult {
                request_id,
                key,
                outcome,
            });
        });
        self.workers.insert(
            request_id,
            DetailWorker {
                cancellation,
                handle,
            },
        );
    }

    fn cancel(&mut self, request_id: u64) {
        if let Some(worker) = self.workers.get(&request_id) {
            worker.cancellation.cancel();
        }
        self.reap_finished();
    }

    fn try_next(&mut self) -> Option<DetailResult> {
        self.reap_finished();
        self.receiver.try_recv().ok()
    }

    fn shutdown(&mut self) {
        for worker in self.workers.values() {
            worker.cancellation.cancel();
        }
        for (_, worker) in self.workers.drain() {
            let _ = worker.handle.join();
        }
    }
}

impl Drop for DetailSession {
    fn drop(&mut self) {
        self.shutdown();
    }
}
