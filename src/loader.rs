use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::day::DaySelection;
use crate::event::LoaderEventSource;
use crate::github::{CancellationToken, CommandRunner, GitHubLoader, LoadEvent};

const EVENT_CHANNEL_CAPACITY: usize = 64;

pub struct LoaderSession {
    receiver: Option<Receiver<LoadEvent>>,
    cancellation: CancellationToken,
    worker: Option<JoinHandle<()>>,
}

impl LoaderSession {
    pub fn start(selection: DaySelection) -> Self {
        let (sender, receiver) = sync_channel(EVENT_CHANNEL_CAPACITY);
        let cancellation = CancellationToken::default();
        let worker_cancellation = cancellation.clone();
        let worker = thread::spawn(move || {
            let loader = GitHubLoader::new(CommandRunner);
            let event_cancellation = worker_cancellation.clone();
            let _ = loader.load_cancellable(&selection, &worker_cancellation, |event| {
                send_event(&sender, &event_cancellation, event);
            });
        });

        Self {
            receiver: Some(receiver),
            cancellation,
            worker: Some(worker),
        }
    }

    pub fn shutdown(&mut self) {
        self.cancellation.cancel();
        self.receiver.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl LoaderEventSource for LoaderSession {
    fn try_next(&mut self) -> Option<LoadEvent> {
        self.receiver.as_ref()?.try_recv().ok()
    }

    fn cancel(&mut self) {
        self.cancellation.cancel();
    }
}

impl Drop for LoaderSession {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn send_event(
    sender: &SyncSender<LoadEvent>,
    cancellation: &CancellationToken,
    mut event: LoadEvent,
) {
    loop {
        if cancellation.is_cancelled() {
            return;
        }
        match sender.try_send(event) {
            Ok(()) | Err(TrySendError::Disconnected(_)) => return,
            Err(TrySendError::Full(returned)) => {
                event = returned;
                thread::sleep(Duration::from_millis(5));
            }
        }
    }
}
