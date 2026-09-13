use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};

use crate::app::{CommentEffect, CommentResult, CommentResultOutcome};
use crate::comment_draft::CommentAnchor;
use crate::event::CommentRequester;
use crate::github::{CancellationToken, CommandRunner, GitHubLoader};

struct Worker {
    cancellation: CancellationToken,
    handle: JoinHandle<()>,
}

pub struct CommentSession {
    sender: Sender<CommentResult>,
    receiver: Receiver<CommentResult>,
    workers: HashMap<u64, Worker>,
}

impl CommentSession {
    pub fn new() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            sender,
            receiver,
            workers: HashMap::new(),
        }
    }

    fn reap(&mut self) {
        let finished = self
            .workers
            .iter()
            .filter_map(|(id, worker)| worker.handle.is_finished().then_some(*id))
            .collect::<Vec<_>>();
        for id in finished {
            if let Some(worker) = self.workers.remove(&id) {
                let _ = worker.handle.join();
            }
        }
    }
}

impl Default for CommentSession {
    fn default() -> Self {
        Self::new()
    }
}

impl CommentRequester for CommentSession {
    fn request(&mut self, effect: CommentEffect) {
        self.reap();
        let (request_id, cancellation, handle) = match effect {
            CommentEffect::Load {
                request_id,
                key,
                repository,
                marker,
                reconcile_target,
            } => {
                if self.workers.contains_key(&request_id) {
                    return;
                }
                let cancellation = CancellationToken::default();
                let worker_token = cancellation.clone();
                let sender = self.sender.clone();
                let result_key = key.clone();
                let result_target = reconcile_target.clone();
                let handle = thread::spawn(move || {
                    let outcome = GitHubLoader::new(CommandRunner).list_commit_comments(
                        &repository,
                        &result_key.sha,
                        marker.as_deref(),
                        &worker_token,
                    );
                    let _ = sender.send(CommentResult {
                        request_id,
                        key: result_key,
                        target: result_target,
                        outcome: CommentResultOutcome::Loaded(outcome),
                    });
                });
                (request_id, cancellation, handle)
            }
            CommentEffect::Publish {
                request_id,
                key,
                target,
                repository,
                body_with_marker,
            } => {
                if self.workers.contains_key(&request_id) {
                    return;
                }
                let cancellation = CancellationToken::default();
                let worker_token = cancellation.clone();
                let sender = self.sender.clone();
                let result_key = key.clone();
                let result_target = target.clone();
                let line = match target.anchor() {
                    CommentAnchor::Commit => None,
                    CommentAnchor::Line(line) => Some((line.path.clone(), line.position)),
                };
                let handle = thread::spawn(move || {
                    let line_ref = line
                        .as_ref()
                        .map(|(path, position)| (path.as_str(), *position));
                    let outcome = GitHubLoader::new(CommandRunner).create_commit_comment(
                        &repository,
                        &result_key.sha,
                        &body_with_marker,
                        line_ref,
                        &worker_token,
                    );
                    let _ = sender.send(CommentResult {
                        request_id,
                        key: result_key,
                        target: Some(result_target),
                        outcome: CommentResultOutcome::Published(outcome),
                    });
                });
                (request_id, cancellation, handle)
            }
            CommentEffect::Cancel { request_id } => {
                self.cancel(request_id);
                return;
            }
        };
        self.workers.insert(
            request_id,
            Worker {
                cancellation,
                handle,
            },
        );
    }

    fn cancel(&mut self, request_id: u64) {
        if let Some(worker) = self.workers.get(&request_id) {
            worker.cancellation.cancel();
        }
        self.reap();
    }
    fn try_next(&mut self) -> Option<CommentResult> {
        self.reap();
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

impl Drop for CommentSession {
    fn drop(&mut self) {
        self.shutdown();
    }
}
