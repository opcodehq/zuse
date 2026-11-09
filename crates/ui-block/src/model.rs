//! The typed component tree renderers consume. Serialized with the component
//! name as the `component` tag and camelCase keys, e.g.
//! `{"component":"Stat","label":"Builds","value":"42"}`.
//!
//! Only [`crate::parse`] guarantees a tree satisfies every limit; a tree
//! deserialized from elsewhere is trusted as-is.

use serde::{Deserialize, Serialize};

/// Visual emphasis for badges and callouts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tone {
    Neutral,
    Good,
    Warn,
    Bad,
}

impl Tone {
    pub(crate) fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "neutral" => Self::Neutral,
            "good" => Self::Good,
            "warn" => Self::Warn,
            "bad" => Self::Bad,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StepStatus {
    Done,
    Active,
    Pending,
    Failed,
}

impl StepStatus {
    pub(crate) fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "done" => Self::Done,
            "active" => Self::Active,
            "pending" => Self::Pending,
            "failed" => Self::Failed,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyValuePair {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChartPoint {
    pub label: String,
    pub value: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    pub label: String,
    pub status: StepStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowNode {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub highlight: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tab {
    pub label: String,
    pub content: Component,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FollowUp {
    pub label: String,
    pub prompt: String,
}

/// One UI block component. Optional arguments are `None` (or `false` /
/// empty) when the spec omitted them or passed `null`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "component", rename_all_fields = "camelCase")]
pub enum Component {
    Card {
        #[serde(default)]
        children: Vec<Component>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
    Text {
        text: String,
    },
    Stat {
        label: String,
        value: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hint: Option<String>,
    },
    KeyValue {
        pairs: Vec<KeyValuePair>,
    },
    /// Every row has exactly `columns.len()` cells.
    Table {
        columns: Vec<String>,
        rows: Vec<Vec<String>>,
    },
    /// `percent` is within 0–100.
    Progress {
        label: String,
        percent: f64,
    },
    Badge {
        label: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tone: Option<Tone>,
    },
    List {
        items: Vec<String>,
    },
    /// `columns` is 1–4; renderers default to 2.
    Grid {
        children: Vec<Component>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        columns: Option<u8>,
    },
    BarChart {
        title: String,
        points: Vec<ChartPoint>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unit: Option<String>,
    },
    LineChart {
        title: String,
        points: Vec<ChartPoint>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unit: Option<String>,
    },
    /// Values are non-negative with a finite total (which may be zero).
    DonutChart {
        title: String,
        points: Vec<ChartPoint>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unit: Option<String>,
    },
    Callout {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tone: Option<Tone>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
    Steps {
        items: Vec<Step>,
    },
    Flow {
        nodes: Vec<FlowNode>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        caption: Option<String>,
    },
    Tabs {
        tabs: Vec<Tab>,
    },
    /// Legacy: still renders from saved transcripts but is not advertised.
    FollowUps {
        items: Vec<FollowUp>,
    },
    Form {
        children: Vec<Component>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        submit_label: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
    Input {
        name: String,
        label: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        placeholder: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        value: Option<String>,
    },
    TextArea {
        name: String,
        label: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        placeholder: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        value: Option<String>,
    },
    /// `value`, when set, is one of `options`.
    Select {
        name: String,
        label: String,
        options: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        value: Option<String>,
    },
    /// `value`, when set, is one of `options`.
    RadioGroup {
        name: String,
        label: String,
        options: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        value: Option<String>,
    },
    Checkbox {
        name: String,
        label: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        checked: bool,
    },
    /// `min < max`, `value` within range, `step > 0`; renderers default
    /// `value` to `min` and `step` to 1.
    Slider {
        name: String,
        label: String,
        min: f64,
        max: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        value: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        step: Option<f64>,
    },
}

impl Component {
    /// The component's spec name, e.g. `"BarChart"`.
    pub fn name(&self) -> &'static str {
        match self {
            Component::Card { .. } => "Card",
            Component::Text { .. } => "Text",
            Component::Stat { .. } => "Stat",
            Component::KeyValue { .. } => "KeyValue",
            Component::Table { .. } => "Table",
            Component::Progress { .. } => "Progress",
            Component::Badge { .. } => "Badge",
            Component::List { .. } => "List",
            Component::Grid { .. } => "Grid",
            Component::BarChart { .. } => "BarChart",
            Component::LineChart { .. } => "LineChart",
            Component::DonutChart { .. } => "DonutChart",
            Component::Callout { .. } => "Callout",
            Component::Steps { .. } => "Steps",
            Component::Flow { .. } => "Flow",
            Component::Tabs { .. } => "Tabs",
            Component::FollowUps { .. } => "FollowUps",
            Component::Form { .. } => "Form",
            Component::Input { .. } => "Input",
            Component::TextArea { .. } => "TextArea",
            Component::Select { .. } => "Select",
            Component::RadioGroup { .. } => "RadioGroup",
            Component::Checkbox { .. } => "Checkbox",
            Component::Slider { .. } => "Slider",
        }
    }

    /// True for components that collect input (only valid inside a Form).
    pub fn is_field(&self) -> bool {
        self.field_info().is_some()
    }

    /// `(name, label)` of a form field component.
    pub fn field_info(&self) -> Option<(&str, &str)> {
        match self {
            Component::Input { name, label, .. }
            | Component::TextArea { name, label, .. }
            | Component::Select { name, label, .. }
            | Component::RadioGroup { name, label, .. }
            | Component::Checkbox { name, label, .. }
            | Component::Slider { name, label, .. } => Some((name, label)),
            _ => None,
        }
    }

    /// Directly nested components: container children or tab contents.
    pub fn children(&self) -> Vec<&Component> {
        match self {
            Component::Card { children, .. }
            | Component::Grid { children, .. }
            | Component::Form { children, .. } => children.iter().collect(),
            Component::Tabs { tabs } => tabs.iter().map(|tab| &tab.content).collect(),
            _ => Vec::new(),
        }
    }
}
