use serde_json::Value;
use std::collections::{HashMap, HashSet};
use zuse_harness::{AgentRoute, Event};
use zuse_protocol::{AgentEvent, DoneStatus, HarnessId, TodoItem, TodoStatus, ToolCall};
pub(super) struct Mapper {
    pub root: String,
    pub cwd: String,
    pub session: String,
    pub steers: HashSet<String>,
    pub replays: HashSet<String>,
    children: HashSet<String>,
    items: HashMap<String, String>,
}
impl Mapper {
    pub fn new(root: String, cwd: String, session: String) -> Self {
        Self {
            root,
            cwd,
            session,
            steers: HashSet::new(),
            replays: HashSet::new(),
            children: HashSet::new(),
            items: HashMap::new(),
        }
    }
    pub fn agent(event: &Event) -> &str {
        match event {
            Event::UserMessage { agent_id, .. }
            | Event::ModelCompleted { agent_id, .. }
            | Event::MailConsumed { agent_id, .. }
            | Event::Text { agent_id, .. }
            | Event::Thinking { agent_id, .. }
            | Event::ToolStart { agent_id, .. }
            | Event::ToolResult { agent_id, .. }
            | Event::Usage { agent_id, .. }
            | Event::RequestAttempt { agent_id, .. }
            | Event::Context { agent_id, .. }
            | Event::Compaction { agent_id, .. }
            | Event::Paused { agent_id, .. }
            | Event::Completed { agent_id, .. } => agent_id,
        }
    }
    fn route(&self, mut event: AgentEvent, route: &AgentRoute) -> AgentEvent {
        for call in &route.parents {
            event = AgentEvent::Subagent {
                parent_tool_use_id: call.clone(),
                event: Box::new(event),
            };
        }
        event
    }
    pub fn translate(
        &mut self,
        event: Event,
        route: &AgentRoute,
        stopped: bool,
    ) -> Vec<AgentEvent> {
        let agent = Self::agent(&event).to_owned();
        let mut result = vec![];
        if agent != self.root && self.children.insert(agent.clone()) {
            result.push(AgentEvent::SessionStarted {
                harness: HarnessId::Zuse,
                model: route.model.clone(),
                tools: vec![],
                cwd: self.cwd.clone(),
                session_id: format!("{}:{agent}", self.session),
                assistant_message_id: uuid::Uuid::new_v4().to_string(),
            });
        }
        match event {
            Event::UserMessage { text, .. } => result.push(AgentEvent::UserMessage { text }),
            Event::Text { item_id, text, .. } => {
                self.items.insert(agent.clone(), item_id);
                result.push(AgentEvent::TextDelta { text });
            }
            Event::Thinking { item_id, text, .. } => {
                self.items.insert(agent.clone(), item_id);
                result.push(AgentEvent::ReasoningDelta { text });
            }
            Event::ModelCompleted { item_id, .. } => {
                self.items.remove(&agent);
                result.push(AgentEvent::AssistantMessageCompleted {
                    assistant_message_id: item_id,
                });
            }
            Event::MailConsumed { delivery_id, .. } => {
                if self.replays.remove(&delivery_id) {
                    result.push(AgentEvent::DeliveryAcknowledged {
                        message_id: delivery_id,
                    });
                } else if self.steers.remove(&delivery_id) {
                    result.push(AgentEvent::Steered {
                        assistant_message_id: self.items.remove(&agent),
                        next_assistant_message_id: Some(uuid::Uuid::new_v4().to_string()),
                    });
                }
            }
            Event::ToolStart { call, .. } => result.push(AgentEvent::ToolCall {
                id: call.id,
                call: tool(&call.name, call.input),
            }),
            Event::ToolResult {
                call_id, output, ..
            } => {
                let text = match output {
                    zuse_harness::ToolOutput::Text(text) => text,
                    other => serde_json::to_string(&other).unwrap_or_default(),
                };
                result.push(AgentEvent::ToolResult {
                    id: call_id,
                    is_error: text.starts_with("Tool failed:"),
                    output: Some(text.chars().take(16000).collect()),
                    diff: None,
                });
            }
            Event::RequestAttempt { attempt, .. } => result.push(AgentEvent::RequestUsage {
                details: serde_json::to_value(attempt).expect("request accounting serializes"),
            }),
            Event::Usage { usage, .. } => result.push(AgentEvent::Usage {
                input_tokens: usage.input_tokens,
                output_tokens: usage.output_tokens,
            }),
            Event::Context {
                used_tokens,
                window_tokens,
                ..
            } => result.push(AgentEvent::ContextUsage {
                tokens: Some(used_tokens),
                window: Some(window_tokens),
            }),
            Event::Compaction {
                item_id,
                status,
                before,
                after,
                ..
            } => {
                let id = format!("compaction:{item_id}");
                if status == "in_progress" {
                    result.push(AgentEvent::ToolCall {
                        id,
                        call: ToolCall::Unknown {
                            name: "Compact context".into(),
                            input: None,
                        },
                    });
                } else {
                    result.push(AgentEvent::ToolResult {
                        id,
                        is_error: status == "failed",
                        output: Some(match after {
                            Some(after) => format!(
                                "Context reduced from {before} to {after} estimated tokens."
                            ),
                            None => {
                                "Context compaction failed; the durable conversation is preserved."
                                    .into()
                            }
                        }),
                        diff: None,
                    });
                }
            }
            Event::Paused { reason, .. } => result.push(AgentEvent::Done {
                status: if stopped || reason == "Interrupted" {
                    DoneStatus::Interrupted
                } else {
                    DoneStatus::Errored
                },
                result: None,
                error: (!stopped && reason != "Interrupted").then_some(reason),
                session_id: Some(if agent == self.root {
                    self.session.clone()
                } else {
                    format!("{}:{agent}", self.session)
                }),
            }),
            Event::Completed { summary, .. } => result.push(AgentEvent::Done {
                status: DoneStatus::Completed,
                result: Some(summary),
                error: None,
                session_id: Some(if agent == self.root {
                    self.session.clone()
                } else {
                    format!("{}:{agent}", self.session)
                }),
            }),
        }
        result
            .into_iter()
            .map(|event| self.route(event, route))
            .collect()
    }
}
fn tool(name: &str, input: Value) -> ToolCall {
    let text = |key: &str| input[key].as_str().unwrap_or_default().to_owned();
    match name {
        "exec_command" => ToolCall::Exec {
            command: text("command"),
        },
        "read_file" | "read_image" | "list_directory" => ToolCall::ReadFile { path: text("path") },
        "write_file" => ToolCall::WriteFile {
            path: text("path"),
            content: Some(text("content")),
        },
        "edit_file" => ToolCall::EditFile {
            path: text("path"),
            old_string: Some(text("old_string")),
            new_string: Some(text("new_string")),
        },
        "multi_edit" => ToolCall::ApplyPatch {
            path: Some(text("path")),
        },
        "grep" => ToolCall::Search {
            pattern: text("pattern"),
            path: input["cwd"].as_str().map(str::to_owned),
        },
        "glob" => ToolCall::Glob {
            pattern: text("pattern"),
        },
        "update_plan" => ToolCall::Todo {
            items: input["steps"]
                .as_array()
                .map(|steps| {
                    steps
                        .iter()
                        .map(|step| {
                            TodoItem::new(
                                step["step"].as_str().unwrap_or_default(),
                                TodoStatus::parse(step["status"].as_str().unwrap_or_default()),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default(),
        },
        "spawn_agent" => ToolCall::Unknown {
            name: format!("Agent: {}", input["name"].as_str().unwrap_or("worker")),
            input: Some(input),
        },
        name if name.starts_with("zuse_") => ToolCall::Mcp {
            server: "zuse".into(),
            tool: name.trim_start_matches("zuse_").into(),
            input: Some(input),
        },
        _ => ToolCall::Unknown {
            name: name.into(),
            input: Some(input),
        },
    }
}
