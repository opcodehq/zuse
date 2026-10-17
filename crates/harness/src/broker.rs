//! One account affinity and health authority for roots and their children.
use crate::{Error, Message, ModelInput, ModelOutput};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Connection {
    pub id: String,
    pub authorized: bool,
    pub preferred: bool,
}
#[derive(Debug, Clone)]
pub enum Health {
    Exhausted { resets_at: Option<u64> },
    Authentication,
    Disconnected,
}
#[async_trait]
pub trait BrokerHost: Send + Sync {
    async fn connections(&self, input: &ModelInput) -> Result<Vec<Connection>, Error>;
    async fn request(&self, connection_id: &str, input: ModelInput) -> Result<ModelOutput, Error>;
    fn switched(&self, root_id: &str, previous: Option<&str>, next: &str);
    fn now(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }
}
#[derive(Default)]
struct State {
    health: HashMap<String, Health>,
    affinity: HashMap<String, String>,
}
pub struct AccountBroker {
    host: Arc<dyn BrokerHost>,
    state: Mutex<State>,
}
impl AccountBroker {
    pub fn new(host: Arc<dyn BrokerHost>) -> Self {
        Self {
            host,
            state: Mutex::new(State::default()),
        }
    }
    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
    pub fn invalidate(&self, id: String, health: Health) {
        self.state().health.insert(id, health);
    }
    pub fn remove_root(&self, id: &str) {
        self.state().affinity.remove(id);
    }
    pub fn forget(&self, id: &str) {
        let mut state = self.state();
        state.health.remove(id);
        state.affinity.retain(|_, active| active != id);
    }
    pub async fn request(&self, input: ModelInput) -> Result<ModelOutput, Error> {
        let mut attempted = HashSet::new();
        let mut physical = 0;
        loop {
            if input.cancel.is_cancelled() {
                return Err(Error::Interrupted);
            }
            let connections = self.host.connections(&input).await?;
            let (connection, previous) = {
                let mut state = self.state();
                let now = self.host.now();
                state
                    .health
                    .retain(|_, h| !matches!(h,Health::Exhausted{resets_at:Some(at)} if *at<=now));
                let eligible = connections
                    .iter()
                    .filter(|c| {
                        c.authorized
                            && !attempted.contains(&c.id)
                            && !state.health.contains_key(&c.id)
                    })
                    .collect::<Vec<_>>();
                let previous = state.affinity.get(&input.root_id).cloned();
                let connection = eligible
                    .iter()
                    .copied()
                    .find(|c| Some(&c.id) == previous.as_ref())
                    .or_else(|| eligible.iter().copied().find(|c| c.preferred))
                    .or_else(|| eligible.first().copied())
                    .ok_or(Error::AccountsUnavailable)?
                    .clone();
                state
                    .affinity
                    .insert(input.root_id.clone(), connection.id.clone());
                (connection, previous)
            };
            attempted.insert(connection.id.clone());
            if previous.as_deref() != Some(&connection.id) {
                self.host
                    .switched(&input.root_id, previous.as_deref(), &connection.id);
            }
            for retry in 0..3 {
                if physical > 0 {
                    input.budget.reserve().await?;
                }
                physical += 1;
                if input.cancel.is_cancelled() {
                    return Err(Error::Interrupted);
                }
                let mut portable = input.clone();
                portable.messages = portable_history(&input.messages);
                let result = tokio::select! {biased;_=input.cancel.cancelled()=>return Err(Error::Interrupted),result=self.host.request(&connection.id,portable)=>result};
                match result {
                    Ok(result) => return Ok(result),
                    Err(Error::Model { code, .. })
                        if code == "subscription_sharing_usage_limit_exceeded" =>
                    {
                        self.invalidate(
                            connection.id.clone(),
                            Health::Exhausted { resets_at: None },
                        );
                        break;
                    }
                    Err(
                        error @ Error::Model {
                            status: Some(401), ..
                        },
                    ) => {
                        self.invalidate(connection.id.clone(), Health::Authentication);
                        return Err(error);
                    }
                    Err(Error::Model { ref code, status })
                        if status == Some(502)
                            || status == Some(503)
                            || code == "subscription_sharing_unavailable" =>
                    {
                        if retry == 2 {
                            return Err(Error::Model {
                                code: code.clone(),
                                status,
                            });
                        }
                        tokio::select! {biased;_=input.cancel.cancelled()=>return Err(Error::Interrupted),_=tokio::time::sleep(Duration::from_millis(250*(1<<retry)))=>{}}
                    }
                    Err(error) => return Err(error),
                }
            }
        }
    }
}
/// Never send account-bound encrypted reasoning or provider item ids to another
/// account. Tool outcomes and user intent remain portable.
pub fn portable_history(messages: &[Message]) -> Vec<Message> {
    messages
        .iter()
        .cloned()
        .map(|mut m| {
            m.metadata.remove("providerOptions");
            if let Some(parts) = m.content.as_array_mut() {
                parts.retain(|p| p["type"] != "reasoning");
                for part in parts {
                    if let Some(map) = part.as_object_mut() {
                        map.remove("providerOptions");
                    }
                }
            }
            m
        })
        .collect()
}
