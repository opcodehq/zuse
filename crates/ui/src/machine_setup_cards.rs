//! Live state for setup chats' `MachineImageProposal` cards: each proposal
//! with the account's machine, read through the engine, and the owner's
//! Apply & rebuild / Discard. One app-wide entity, so every transcript showing
//! a proposal shares the same reads; it polls only while an applied
//! proposal's image is waiting or building.

use crate::state::AppState;
use gpui::*;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use zuse_protocol::machine_setup::ProposalView;
use zuse_rpc::methods;
use zuse_viewport::ui_block::{MachineSetupAction, MachineSetupDirectory, ProposalCard};

/// How often a building image is reread while a card shows it.
const BUILD_POLL: std::time::Duration = std::time::Duration::from_secs(5);

pub struct MachineSetupCards {
    state: Entity<AppState>,
    cards: HashMap<String, ProposalCard>,
    /// Proposals asked for, so a re-render never refetches.
    requested: HashSet<String>,
    poll: Option<Task<()>>,
    tasks: Vec<Task<()>>,
}

struct Shared(Entity<MachineSetupCards>);
impl Global for Shared {}

fn owner(state: &AppState) -> Option<(String, Option<String>)> {
    Some((
        state.cloud_user_id()?.to_owned(),
        state.cloud_account.as_ref()?["orgId"]
            .as_str()
            .map(str::to_owned),
    ))
}

/// Plain words for a refused apply or discard.
pub fn error_copy(error: &str) -> String {
    let known = [
        (
            "machine_setup_repository_unavailable",
            "A repository isn't reachable through your GitHub connection. Grant access in Settings → Cloud computer → Repositories, then apply again.",
        ),
        (
            "machine_setup_github_required",
            "Connect GitHub in Settings → Cloud computer → Repositories to add repositories.",
        ),
        (
            "machine_setup_proposal_stale",
            "Your setup script changed since this was proposed. Ask the agent to propose again.",
        ),
        (
            "machine_setup_proposal_superseded",
            "A newer proposal replaced this one.",
        ),
        (
            "machine_revision_changed",
            "Your machine settings changed meanwhile. Ask the agent to propose again.",
        ),
        (
            "cloud_billing",
            "Image builds use cloud compute. Check your plan to rebuild.",
        ),
        (
            "shared_credit_exhausted",
            "Image builds use cloud compute. Check your plan to rebuild.",
        ),
        (
            "forbidden",
            "Only the account owner or an organization admin can change the machine image.",
        ),
    ];
    known
        .iter()
        .find(|(code, _)| error.contains(code))
        .map(|(_, text)| (*text).to_owned())
        .unwrap_or_else(|| "The image change didn't go through. Try again.".to_owned())
}

impl MachineSetupCards {
    pub fn shared(state: Entity<AppState>, cx: &mut App) -> Entity<Self> {
        if let Some(shared) = cx.try_global::<Shared>() {
            return shared.0.clone();
        }
        let cards = cx.new(|_| Self {
            state,
            cards: HashMap::new(),
            requested: HashSet::new(),
            poll: None,
            tasks: Vec::new(),
        });
        cx.set_global(Shared(cards.clone()));
        cards
    }

    /// Review fixtures: show `view` for its proposal without reading it.
    pub fn fixture_seed(state: Entity<AppState>, view: ProposalView, cx: &mut App) {
        let cards = Self::shared(state, cx);
        cards.update(cx, |cards, cx| {
            let id = view.proposal.proposal_id.clone();
            cards.requested.insert(id.clone());
            cards.cards.insert(
                id,
                ProposalCard {
                    view: Some(view),
                    busy: false,
                    error: None,
                },
            );
            cx.notify();
        });
    }

    pub fn directory(&self) -> MachineSetupDirectory {
        MachineSetupDirectory {
            proposals: self.cards.clone(),
            unavailable: false,
        }
    }

    /// Read each of `ids` once; later changes arrive by action or poll.
    pub fn ensure(&mut self, ids: &[String], cx: &mut Context<Self>) {
        for id in ids {
            if self.requested.insert(id.clone()) {
                self.call(id, methods::CLOUD_MACHINE_SETUP_PROPOSAL, cx);
            }
        }
    }

    pub fn act(&mut self, action: &MachineSetupAction, cx: &mut Context<Self>) {
        let (id, method) = match action {
            MachineSetupAction::Apply { proposal } => {
                (proposal, methods::CLOUD_MACHINE_SETUP_APPLY)
            }
            MachineSetupAction::Discard { proposal } => {
                (proposal, methods::CLOUD_MACHINE_SETUP_DISCARD)
            }
        };
        if self.cards.get(id).is_some_and(|c| c.busy) {
            return;
        }
        self.call(id, method, cx);
    }

    fn call(&mut self, id: &str, method: &'static str, cx: &mut Context<Self>) {
        let card = self.cards.entry(id.to_owned()).or_default();
        card.busy = true;
        card.error = None;
        cx.notify();
        let state = self.state.read(cx);
        let (Some(engine), Some(owner)) = (state.engine().map(|e| e.controller()), owner(state))
        else {
            let card = self.cards.entry(id.to_owned()).or_default();
            card.busy = false;
            card.error = Some("Sign in to Zuse to review this image change.".into());
            return;
        };
        let params =
            json!({"proposalId":id,"expectedAccountId":owner.0,"expectedOrganizationId":owner.1});
        let job = gpui_tokio::Tokio::spawn(cx, async move {
            engine
                .client()
                .call_scoped(method, params)
                .await
                .map_err(|e| e.to_string())
        });
        let id = id.to_owned();
        let read = method == methods::CLOUD_MACHINE_SETUP_PROPOSAL;
        self.tasks.retain(|task| !task.is_ready());
        self.tasks.push(cx.spawn(async move |this, cx| {
            let result = job.await.unwrap_or_else(|e| Err(e.to_string()));
            let _ = this.update(cx, |this, cx| this.receive(&id, read, result, cx));
        }));
    }

    fn receive(
        &mut self,
        id: &str,
        read: bool,
        result: Result<Value, String>,
        cx: &mut Context<Self>,
    ) {
        let card = self.cards.entry(id.to_owned()).or_default();
        card.busy = false;
        let value = result.map(|v| {
            if v.get("data").is_some() {
                v["data"].clone()
            } else {
                v
            }
        });
        match value.map(serde_json::from_value::<ProposalView>) {
            Ok(Ok(view)) => {
                card.view = Some(view);
                card.error = None;
            }
            Ok(Err(_)) => card.error = Some("This image change couldn't be read.".into()),
            Err(error) if read && card.view.is_none() => {
                card.error = Some(if error.contains("not_found") {
                    "This proposal belongs to another account.".into()
                } else {
                    "This image change couldn't be loaded.".into()
                });
                // A later render may try again.
                self.requested.remove(id);
            }
            Err(error) => card.error = Some(error_copy(&error)),
        }
        self.schedule_poll(cx);
        cx.notify();
    }

    /// Reread applied proposals while their image waits or builds.
    fn schedule_poll(&mut self, cx: &mut Context<Self>) {
        let building: Vec<String> = self
            .cards
            .iter()
            .filter(|(_, card)| {
                !card.busy
                    && card.view.as_ref().is_some_and(|v| {
                        v.proposal.state == "applied"
                            && matches!(v.machine.image_state.as_str(), "queued" | "building")
                    })
            })
            .map(|(id, _)| id.clone())
            .collect();
        if building.is_empty() {
            self.poll = None;
            return;
        }
        if self.poll.is_some() {
            return;
        }
        self.poll = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(BUILD_POLL).await;
            let _ = this.update(cx, |this, cx| {
                this.poll = None;
                for id in &building {
                    this.call(id, methods::CLOUD_MACHINE_SETUP_PROPOSAL, cx);
                }
            });
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::error_copy;

    #[test]
    fn refusals_read_as_next_steps() {
        assert!(error_copy("409 machine_setup_repository_unavailable").contains("GitHub"));
        assert!(error_copy("machine_setup_proposal_stale").contains("propose again"));
        assert!(error_copy("cloud_billing_hold").contains("plan"));
        assert_eq!(
            error_copy("boom"),
            "The image change didn't go through. Try again."
        );
    }
}
