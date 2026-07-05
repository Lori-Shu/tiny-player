use std::sync::Arc;

use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use tracing::info;

#[derive(Debug)]
pub struct AsyncCleaner {
    transcribe_task_cancel_token: Option<Arc<CancellationToken>>,
    transcribe_task_notify: Option<Arc<Notify>>,
}
impl AsyncCleaner {
    pub fn new() -> Self {
        Self {
            transcribe_task_cancel_token: None,
            transcribe_task_notify: None,
        }
    }
    pub fn add_transcriber_resources(
        &mut self,
        transcribe_task_cancel_token: Arc<CancellationToken>,
        transcribe_task_notify: Arc<Notify>,
    ) {
        self.transcribe_task_cancel_token = Some(transcribe_task_cancel_token);
        self.transcribe_task_notify = Some(transcribe_task_notify);
    }
    pub fn start_clean(&mut self) {
        if let Some(transcribe_task_cancel_token) = &mut self.transcribe_task_cancel_token {
            transcribe_task_cancel_token.cancel();
            info!("transcribe_task_cancel_token canceled");
        }
        if let Some(transcribe_task_notify) = &mut self.transcribe_task_notify {
            transcribe_task_notify.notify_waiters();
            info!("transcribe_task_notify notified");
        }
    }
}
