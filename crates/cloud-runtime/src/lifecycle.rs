//! Shutdown receipts follow engine retirement and checked durable snapshots.
use crate::Authority;
use anyhow::{Result, ensure};
use std::sync::Arc;
use zuse_engine::{EngineCore, doc_host::ChatDocHandle};
use zuse_protocol::{
    cloud::CloudRuntimeBootstrap,
    cloud_lifecycle::{Action, Directive, RuntimeRequest},
};

fn family(core: &EngineCore, boot: &CloudRuntimeBootstrap) -> Result<Vec<String>> {
    let chats = core.workspace.read_chats()?;
    ensure!(
        chats
            .iter()
            .any(|c| c.id == boot.chat_id && c.device_id == core.device_id),
        "runtime_retained_history_missing"
    );
    let mut ids = std::collections::HashSet::from([boot.chat_id.clone()]);
    loop {
        let before = ids.len();
        for c in &chats {
            if c.device_id == core.device_id
                && (c.parent_chat_id.as_ref().is_some_and(|p| ids.contains(p))
                    || c.thread_id.as_ref().is_some_and(|p| ids.contains(p)))
            {
                ids.insert(c.id.clone());
            }
        }
        if ids.len() == before {
            break;
        }
    }
    Ok(ids.into_iter().collect())
}
pub struct Shutdown {
    directive: Directive,
    handles: Vec<Arc<ChatDocHandle>>,
}
/// Must run before engine shutdown; held handles survive retirement for the final checked flush.
pub fn hold(
    core: &EngineCore,
    boot: &CloudRuntimeBootstrap,
    directive: Directive,
) -> Result<Shutdown> {
    ensure!(
        matches!(directive.action, Action::Pause | Action::Archive)
            && directive.phase == "quiescing",
        "runtime_lifecycle_directive_invalid"
    );
    core.doc_host.pause_all_queues();
    let mut handles = Vec::new();
    for id in family(core, boot)? {
        core.doc_host.pause_chat_queue(&id)?;
        handles.push(core.doc_host.open_local(&id)?);
        if directive.action == Action::Archive {
            core.workspace.set_chat_archived(&id, true)?;
        }
    }
    Ok(Shutdown { directive, handles })
}
impl Shutdown {
    /// Call only after EngineCore::shutdown has awaited agents, PTYs and doc workers.
    pub async fn acknowledge(
        self,
        core: &EngineCore,
        authority: &Authority,
        boot: &CloudRuntimeBootstrap,
    ) -> Result<()> {
        for handle in self.handles {
            core.doc_host.flush_chat_checked(&handle)?;
        }
        core.workspace.flush_checked()?;
        authority
            .lifecycle(
                boot,
                RuntimeRequest::Quiesced {
                    operation_id: self.directive.operation_id,
                },
            )
            .await?;
        Ok(())
    }
}
/// Unarchive the original family on the retained disk before any interactive admission.
pub fn activate(core: &EngineCore, boot: &CloudRuntimeBootstrap, d: &Directive) -> Result<()> {
    ensure!(
        d.action == Action::Resume && matches!(d.phase.as_str(), "waking" | "launching" | "ready"),
        "runtime_lifecycle_directive_invalid"
    );
    for id in family(core, boot)? {
        core.workspace.set_chat_archived(&id, false)?;
    }
    core.workspace.flush_checked()?;
    Ok(())
}
