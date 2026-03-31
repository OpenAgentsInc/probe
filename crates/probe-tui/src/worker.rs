use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::message::{AppMessage, BackgroundTaskKind, BackgroundTaskRequest};

const TASK_STEP_DELAY: Duration = Duration::from_millis(30);

enum WorkerCommand {
    Run(BackgroundTaskRequest),
    Shutdown,
}

#[derive(Debug)]
pub struct BackgroundWorker {
    command_tx: Sender<WorkerCommand>,
    message_rx: Receiver<AppMessage>,
    join_handle: Option<JoinHandle<()>>,
}

impl BackgroundWorker {
    #[must_use]
    pub fn new() -> Self {
        let (command_tx, command_rx) = mpsc::channel();
        let (message_tx, message_rx) = mpsc::channel();
        let join_handle = thread::Builder::new()
            .name(String::from("probe-tui-worker"))
            .spawn(move || worker_loop(command_rx, message_tx))
            .expect("probe tui worker thread should spawn");
        Self {
            command_tx,
            message_rx,
            join_handle: Some(join_handle),
        }
    }

    pub fn submit(&self, request: BackgroundTaskRequest) -> Result<(), String> {
        self.command_tx
            .send(WorkerCommand::Run(request))
            .map_err(|_| String::from("background worker is unavailable"))
    }

    pub fn try_recv(&self) -> Result<Option<AppMessage>, String> {
        match self.message_rx.try_recv() {
            Ok(message) => Ok(Some(message)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => {
                Err(String::from("background worker disconnected unexpectedly"))
            }
        }
    }
}

impl Default for BackgroundWorker {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for BackgroundWorker {
    fn drop(&mut self) {
        let _ = self.command_tx.send(WorkerCommand::Shutdown);
        if let Some(handle) = self.join_handle.take() {
            let _ = handle.join();
        }
    }
}

fn worker_loop(command_rx: Receiver<WorkerCommand>, message_tx: Sender<AppMessage>) {
    while let Ok(command) = command_rx.recv() {
        match command {
            WorkerCommand::Run(request) => run_demo_task(request, &message_tx),
            WorkerCommand::Shutdown => break,
        }
    }
}

fn run_demo_task(request: BackgroundTaskRequest, message_tx: &Sender<AppMessage>) {
    let kind = request.kind();
    let title = request.title().to_string();

    if message_tx
        .send(AppMessage::TaskStarted {
            kind,
            title: title.clone(),
        })
        .is_err()
    {
        return;
    }

    if message_tx
        .send(AppMessage::TaskProgress {
            kind,
            step: 1,
            total_steps: 2,
            detail: String::from("worker accepted the request and reserved the task lane"),
        })
        .is_err()
    {
        return;
    }
    thread::sleep(TASK_STEP_DELAY);

    if matches!(request, BackgroundTaskRequest::DemoFailure) {
        let _ = message_tx.send(AppMessage::TaskFailed {
            kind,
            title,
            detail: String::from("demo worker injected a controlled failure for testing"),
        });
        return;
    }

    if message_tx
        .send(AppMessage::TaskProgress {
            kind,
            step: 2,
            total_steps: 2,
            detail: String::from("worker completed the fake task payload and packaged the result"),
        })
        .is_err()
    {
        return;
    }
    thread::sleep(TASK_STEP_DELAY);

    let _ = message_tx.send(AppMessage::TaskSucceeded {
        kind: BackgroundTaskKind::ProbeSetupDemo,
        title,
        lines: vec![
            String::from("Background task finished without blocking the UI loop."),
            String::from("The app shell can now ingest typed worker messages on tick."),
            String::from("Issue #32 will swap this fake task for the Apple FM setup flow."),
        ],
    });
}
