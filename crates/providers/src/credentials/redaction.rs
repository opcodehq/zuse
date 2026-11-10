//! Runtime-owned secret filtering before provider events reach journals, docs or
//! tool sidecars. Known values are retained for this runtime's lifetime so a
//! delayed/background result cannot reveal a rotated credential.
use crate::*;
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
};
use zeroize::{Zeroize, Zeroizing};
#[derive(Default)]
pub struct Secrets(Mutex<Vec<Zeroizing<String>>>);
impl Secrets {
    pub fn record(&self, value: &str) -> Result<(), HarnessError> {
        if value.is_empty() {
            return Ok(());
        }
        let mut candidates = vec![Zeroizing::new(value.to_owned())];
        for encoded in [
            serde_json::to_string(value).unwrap_or_default(),
            format!("{value:?}"),
        ] {
            if encoded.len() >= 2 {
                let escaped = &encoded[1..encoded.len() - 1];
                if escaped != value
                    && !escaped.is_empty()
                    && !candidates
                        .iter()
                        .any(|candidate| candidate.as_str() == escaped)
                {
                    candidates.push(Zeroizing::new(escaped.into()));
                }
            }
        }
        let mut values = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        candidates.retain(|candidate| {
            !values
                .iter()
                .any(|existing| existing.as_str() == candidate.as_str())
        });
        if value.len() > 65536
            || values.len() + candidates.len() > 256
            || values.iter().map(|value| value.len()).sum::<usize>()
                + candidates.iter().map(|value| value.len()).sum::<usize>()
                > 2 * 1024 * 1024
        {
            return Err(HarnessError::Protocol(
                "secret-filter-capacity-requires-runtime-restart".into(),
            ));
        }
        values.extend(candidates);
        Ok(())
    }
    pub fn contains(&self, text: &str) -> bool {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .any(|secret| text.contains(secret.as_str()))
    }
    /// Answer options/prefills are operational values. Reject unsafe choices
    /// rather than silently changing the answer the driver will receive.
    pub fn questions(self: &Arc<Self>, questions: &mut [UserInputQuestion]) -> bool {
        if questions.iter().any(|question| {
            question.options.iter().any(|option| self.contains(option))
                || question
                    .prefill
                    .as_deref()
                    .is_some_and(|prefill| self.contains(prefill))
        }) {
            return false;
        }
        for question in questions {
            question.header = self.mask(&question.header);
            question.question = self.mask(&question.question);
        }
        true
    }
    pub fn mask(self: &Arc<Self>, text: &str) -> String {
        let mut filter = Text::new(self.clone());
        let mut out = filter.push(text.as_bytes());
        out.extend(filter.finish());
        String::from_utf8_lossy(&out).into_owned()
    }
    pub fn error(self: &Arc<Self>, error: HarnessError) -> HarnessError {
        match error {
            HarnessError::NotInstalled(message) => HarnessError::NotInstalled(self.mask(&message)),
            HarnessError::Protocol(message) => HarnessError::Protocol(self.mask(&message)),
            HarnessError::Install(message) => HarnessError::Install(self.mask(&message)),
            HarnessError::Discovery(mut failure) => {
                failure.message = self.mask(&failure.message);
                HarnessError::Discovery(failure)
            }
            HarnessError::Io(error) => HarnessError::Io(std::io::Error::new(
                error.kind(),
                self.mask(&error.to_string()),
            )),
        }
    }
}
pub struct Text {
    secrets: Arc<Secrets>,
    pending: Zeroizing<Vec<u8>>,
}
impl Text {
    pub fn new(secrets: Arc<Secrets>) -> Self {
        Self {
            secrets,
            pending: Zeroizing::new(Vec::new()),
        }
    }
    pub fn push(&mut self, bytes: &[u8]) -> Vec<u8> {
        self.pending.extend_from_slice(bytes);
        let mut out = Vec::new();
        let mut at = 0;
        let values = self
            .secrets
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while at < self.pending.len() {
            let rest = &self.pending[at..];
            if let Some(secret) = values
                .iter()
                .filter(|secret| rest.starts_with(secret.as_bytes()))
                .max_by_key(|secret| secret.len())
            {
                out.extend_from_slice(b"[redacted]");
                at += secret.len();
            } else if values
                .iter()
                .any(|secret| secret.as_bytes().starts_with(rest))
            {
                break;
            } else {
                out.push(self.pending[at]);
                at += 1;
            }
        }
        self.pending[..at].zeroize();
        self.pending.drain(..at);
        out
    }
    /// A final possible secret prefix is conservatively hidden, not flushed raw.
    pub fn finish(&mut self) -> Vec<u8> {
        let out = if self.pending.is_empty() {
            vec![]
        } else {
            b"[redacted]".to_vec()
        };
        self.pending.zeroize();
        self.pending.clear();
        out
    }
}
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Text,
    Reasoning,
}
struct Pending {
    parents: Vec<String>,
    kind: Kind,
    text: Text,
}
pub struct Events {
    secrets: Arc<Secrets>,
    pending: HashMap<String, Pending>,
}
fn wrap(mut event: AgentEvent, parents: &[String]) -> AgentEvent {
    for parent in parents.iter().rev() {
        event = AgentEvent::Subagent {
            parent_tool_use_id: parent.clone(),
            event: Box::new(event),
        };
    }
    event
}
impl Events {
    pub fn new(secrets: Arc<Secrets>) -> Self {
        Self {
            secrets,
            pending: HashMap::new(),
        }
    }
    fn flush(&mut self, key: &str) -> Vec<AgentEvent> {
        let Some(mut pending) = self.pending.remove(key) else {
            return vec![];
        };
        let text = String::from_utf8_lossy(&pending.text.finish()).into_owned();
        if text.is_empty() {
            return vec![];
        }
        vec![wrap(
            match pending.kind {
                Kind::Text => AgentEvent::TextDelta { text },
                Kind::Reasoning => AgentEvent::ReasoningDelta { text },
            },
            &pending.parents,
        )]
    }
    pub fn finish(&mut self) -> Vec<AgentEvent> {
        let keys = self.pending.keys().cloned().collect::<Vec<_>>();
        keys.into_iter().flat_map(|key| self.flush(&key)).collect()
    }
    pub fn push(&mut self, event: AgentEvent) -> Vec<AgentEvent> {
        let mut parents = Vec::new();
        let mut leaf = event;
        while let AgentEvent::Subagent {
            parent_tool_use_id,
            event,
        } = leaf
        {
            parents.push(parent_tool_use_id);
            leaf = *event;
        }
        let key = serde_json::to_string(&parents).expect("string path");
        let mut out = Vec::new();
        let delta = match &leaf {
            AgentEvent::TextDelta { .. } => Some(Kind::Text),
            AgentEvent::ReasoningDelta { .. } => Some(Kind::Reasoning),
            _ => None,
        };
        if let Some(kind) = delta {
            if self
                .pending
                .get(&key)
                .is_some_and(|pending| pending.kind != kind)
            {
                out.extend(self.flush(&key));
            }
            if !self.pending.contains_key(&key) && self.pending.len() >= 1024 {
                out.extend(self.finish());
                out.push(wrap(
                    AgentEvent::Error {
                        message: "secret-filter-stream-capacity-exceeded".into(),
                    },
                    &parents,
                ));
                return out;
            }
            let pending = self.pending.entry(key).or_insert_with(|| Pending {
                parents: parents.clone(),
                kind,
                text: Text::new(self.secrets.clone()),
            });
            let text = match leaf {
                AgentEvent::TextDelta { text } | AgentEvent::ReasoningDelta { text } => text,
                _ => unreachable!(),
            };
            let text = String::from_utf8_lossy(&pending.text.push(text.as_bytes())).into_owned();
            out.push(wrap(
                match kind {
                    Kind::Text => AgentEvent::TextDelta { text },
                    Kind::Reasoning => AgentEvent::ReasoningDelta { text },
                },
                &parents,
            ));
            return out;
        }
        if parents.is_empty() && matches!(leaf, AgentEvent::Done { .. }) {
            out.extend(self.finish());
        } else {
            out.extend(self.flush(&key));
        }
        if let AgentEvent::InputRequested { questions, .. } = &mut leaf {
            if !self.secrets.questions(questions) {
                leaf = AgentEvent::Error {
                    message: "Secret values cannot be exposed as input choices".into(),
                };
            }
        }
        self.mask_event(&mut leaf);
        out.push(wrap(leaf, &parents));
        out
    }
    pub fn mask_json(&self, value: &mut serde_json::Value) {
        match value {
            serde_json::Value::String(text) => {
                let masked = self.secrets.mask(text);
                text.zeroize();
                *text = masked;
            }
            serde_json::Value::Array(values) => {
                for value in values {
                    self.mask_json(value)
                }
            }
            serde_json::Value::Object(values) => {
                let old = std::mem::take(values);
                for (mut key, mut value) in old {
                    let masked = self.secrets.mask(&key);
                    key.zeroize();
                    self.mask_json(&mut value);
                    values.insert(masked, value);
                }
            }
            _ => {}
        }
    }
    fn mask_text(&self, text: &mut String) {
        let masked = self.secrets.mask(text);
        text.zeroize();
        *text = masked;
    }
    pub fn mask_event(&self, event: &mut AgentEvent) {
        match event {
            AgentEvent::ToolCall { call, .. } => {
                if let Ok(mut value) = serde_json::to_value(&*call) {
                    if let Some(object) = value.as_object_mut() {
                        for (name, value) in object {
                            if name != "kind" {
                                self.mask_json(value);
                            }
                        }
                    }
                    if let Ok(masked) = serde_json::from_value(value) {
                        *call = masked;
                    } else {
                        *call = zuse_protocol::ToolCall::Unknown {
                            name: "[redacted]".into(),
                            input: None,
                        };
                    }
                }
            }
            AgentEvent::ToolResult { output, diff, .. } => {
                if let Some(output) = output {
                    self.mask_text(output);
                }
                if let Some(diff) = diff {
                    self.mask_text(&mut diff.path);
                    if let Some(old) = &mut diff.old_text {
                        self.mask_text(old);
                    }
                    self.mask_text(&mut diff.new_text);
                }
            }
            AgentEvent::Error { message } => self.mask_text(message),
            AgentEvent::Done { result, error, .. } => {
                if let Some(text) = result {
                    self.mask_text(text);
                }
                if let Some(text) = error {
                    self.mask_text(text);
                }
            }
            AgentEvent::UserMessage { text } => self.mask_text(text),
            AgentEvent::RequestUsage { details } => self.mask_json(details),
            AgentEvent::GeneratedImage { name, .. } => self.mask_text(name),
            AgentEvent::Visual { title, path, .. } => {
                self.mask_text(title);
                self.mask_text(path);
            }
            AgentEvent::AvailableCommands { commands } => {
                for command in commands {
                    self.mask_text(&mut command.description);
                    if let Some(hint) = &mut command.input_hint {
                        self.mask_text(hint);
                    }
                }
            }
            _ => {}
        }
    }
    pub fn stream(
        stream: futures::stream::BoxStream<'static, Result<AgentEvent, HarnessError>>,
        secrets: Arc<Secrets>,
    ) -> futures::stream::BoxStream<'static, Result<AgentEvent, HarnessError>> {
        use futures::StreamExt;
        futures::stream::unfold(
            (stream, Self::new(secrets), VecDeque::new(), false),
            |(mut stream, mut filter, mut queued, mut ended)| async move {
                loop {
                    if let Some(event) = queued.pop_front() {
                        return Some((event, (stream, filter, queued, ended)));
                    }
                    if ended {
                        return None;
                    }
                    match stream.next().await {
                        Some(Ok(event)) => queued.extend(filter.push(event).into_iter().map(Ok)),
                        Some(Err(error)) => {
                            queued.extend(filter.finish().into_iter().map(Ok));
                            queued.push_back(Err(filter.secrets.error(error)));
                            ended = true;
                        }
                        None => {
                            queued.extend(filter.finish().into_iter().map(Ok));
                            ended = true;
                        }
                    }
                }
            },
        )
        .boxed()
    }
}
/// Delegation preserves all driver lifecycle and idempotent-delivery semantics.
pub struct Filtered {
    inner: Arc<dyn Harness>,
    secrets: Arc<Secrets>,
}
impl Filtered {
    pub fn wrap(inner: Arc<dyn Harness>, secrets: Arc<Secrets>) -> Arc<Self> {
        Arc::new(Self { inner, secrets })
    }
    fn events(
        &self,
        result: Result<
            futures::stream::BoxStream<'static, Result<AgentEvent, HarnessError>>,
            HarnessError,
        >,
    ) -> Result<futures::stream::BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError>
    {
        result
            .map(|stream| Events::stream(stream, self.secrets.clone()))
            .map_err(|error| self.secrets.error(error))
    }
}
#[async_trait::async_trait]
impl Harness for Filtered {
    fn id(&self) -> HarnessId {
        self.inner.id()
    }
    fn display_name(&self) -> &str {
        self.inner.display_name()
    }
    fn supports_steering(&self) -> bool {
        self.inner.supports_steering()
    }
    fn steering_mode(&self) -> SteeringMode {
        self.inner.steering_mode()
    }
    fn reasoning_levels(&self) -> &[ReasoningLevel] {
        self.inner.reasoning_levels()
    }
    fn installed(&self) -> bool {
        self.inner.installed()
    }
    fn executable_path(&self) -> Option<std::path::PathBuf> {
        self.inner.executable_path()
    }
    fn deterministic_turn_end(&self) -> bool {
        self.inner.deterministic_turn_end()
    }
    fn authoritative_prompt_end(&self) -> bool {
        self.inner.authoritative_prompt_end()
    }
    fn stops_turn_in_place(&self) -> bool {
        self.inner.stops_turn_in_place()
    }
    fn reconfigures_in_place(&self, live: &RunRequest, next: &RunRequest) -> bool {
        self.inner.reconfigures_in_place(live, next)
    }
    fn same_runtime(&self, live: &RunRequest, next: &RunRequest) -> bool {
        self.inner.same_runtime(live, next)
    }
    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        self.inner
            .models()
            .await
            .map_err(|error| self.secrets.error(error))
    }
    fn model_context(&self) -> Result<Option<ModelContext>, HarnessError> {
        self.inner
            .model_context()
            .map_err(|error| self.secrets.error(error))
    }
    fn fallback_models(&self) -> Vec<Model> {
        self.inner.fallback_models()
    }
    async fn model_catalog(&self, force: bool) -> Result<ModelCatalog, HarnessError> {
        self.inner
            .model_catalog(force)
            .await
            .map_err(|error| self.secrets.error(error))
    }
    async fn commands(&self) -> Result<Vec<SlashCommand>, HarnessError> {
        self.inner
            .commands()
            .await
            .map_err(|error| self.secrets.error(error))
    }
    async fn commands_for(&self, cwd: &std::path::Path) -> Result<Vec<SlashCommand>, HarnessError> {
        self.inner
            .commands_for(cwd)
            .await
            .map_err(|error| self.secrets.error(error))
    }
    async fn skills(
        &self,
        cwd: &std::path::Path,
    ) -> Result<Option<Vec<zuse_protocol::invocation::Skill>>, HarnessError> {
        self.inner
            .skills(cwd)
            .await
            .map_err(|error| self.secrets.error(error))
    }
    async fn run_title(
        &self,
        request: RunRequest,
        controls: RunControls,
    ) -> Result<futures::stream::BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError>
    {
        self.events(self.inner.run_title(request, controls).await)
    }
    async fn start_idle(
        &self,
        request: RunRequest,
        controls: RunControls,
    ) -> Result<futures::stream::BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError>
    {
        self.events(self.inner.start_idle(request, controls).await)
    }
    async fn run_delivery(
        &self,
        request: RunRequest,
        controls: RunControls,
        identity: RunIdentity,
    ) -> Result<futures::stream::BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError>
    {
        self.events(self.inner.run_delivery(request, controls, identity).await)
    }
    async fn run(
        &self,
        request: RunRequest,
        controls: RunControls,
    ) -> Result<futures::stream::BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError>
    {
        self.events(self.inner.run(request, controls).await)
    }
}
