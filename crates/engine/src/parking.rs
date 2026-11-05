//! In-process parking keeps the engine graph, harnesses, services and PTYs owned.
use crate::{EngineCore, EngineError, execution::ExecutionDrain};
use std::time::Duration;

pub struct ParkedExecution {
    drain: ExecutionDrain,
}
impl EngineCore {
    /// Close every shared execution entry point before settling active turns.
    /// The caller must stop ingress/mailbox delivery first and publish the final
    /// checkpoint before acknowledging sleep. This does not suspend a provider.
    pub async fn park_execution(&self) -> Result<ParkedExecution, EngineError> {
        let drain = self.sessions.execution_admission().close()?;
        let settle = async {
            // Stop already-live turns so admitted nested requests can settle.
            self.sessions.settle_retained_turns().await?;
            drain.drained().await;
            // An admitted startup may have completed during the first drain.
            // With no remaining permits and admission closed, the final run set
            // is stable. Stop its turns without terminating persistent runtimes.
            self.sessions.settle_retained_turns().await?;
            drain.drained().await;
            self.doc_host.flush_all_checked()?;
            self.workspace.flush_checked()?;
            Ok::<(), EngineError>(())
        };
        tokio::time::timeout(Duration::from_secs(35), settle)
            .await
            .map_err(|_| EngineError::Other("runtime_execution_drain_timed_out".into()))??;
        Ok(ParkedExecution { drain })
    }
    /// Only after authority renewal and the generation-bound resume handshake.
    /// Restores the same queues and processes; it never touches Git or setup.
    pub fn resume_execution(&self, parked: ParkedExecution) -> Result<(), EngineError> {
        parked.drain.reopen(self.sessions.execution_admission())?;
        self.doc_host.kick_drains();
        Ok(())
    }
}
