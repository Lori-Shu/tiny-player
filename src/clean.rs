use std::process::Child;

use tracing::warn;

#[derive(Debug)]
pub struct ProcessCleaner {
    whisper_child: Child,
}
impl ProcessCleaner {
    pub fn new(whisper_child: Child) -> Self {
        Self { whisper_child }
    }
    pub fn clean(&mut self) {
        if let Err(e) = self.whisper_child.kill() {
            warn!("kill child process err:{}", e);
        }
    }
}
