//! The component catalogue: the single table that drives positional argument
//! mapping, shape validation, and the prompt signature lines. Parameter order
//! IS the positional argument order — required parameters first, optional
//! last; containers take `children` first so `Card([a, b])` reads naturally.

/// Shape of a parameter or object key, with its limits. String lengths are
/// counted in UTF-16 code units so limits match the reference exactly.
#[derive(Debug)]
pub(crate) enum Ty {
    Str {
        min: usize,
        max: Option<usize>,
    },
    /// Form field name: `^[A-Za-z][\w-]{0,39}$`.
    FieldName,
    Num {
        min: Option<f64>,
        max: Option<f64>,
        int: bool,
        positive: bool,
    },
    Bool,
    Enum(&'static [&'static str]),
    Array {
        item: &'static Ty,
        min: usize,
        max: Option<usize>,
    },
    Object(&'static [Param]),
    /// A nested component (shape checked when it is built).
    Component,
}

#[derive(Debug)]
pub(crate) struct Param {
    pub name: &'static str,
    pub required: bool,
    pub ty: Ty,
}

#[derive(Debug)]
pub(crate) struct ComponentDef {
    pub name: &'static str,
    pub description: &'static str,
    /// False keeps saved blocks rendering but hides the component from agents.
    pub advertised: bool,
    /// Collects input; only valid inside a Form.
    pub field: bool,
    pub params: &'static [Param],
}

pub(crate) const TONES: &[&str] = &["neutral", "good", "warn", "bad"];
pub(crate) const STEP_STATUSES: &[&str] = &["done", "active", "pending", "failed"];

const STR: Ty = Ty::Str { min: 0, max: None };
const NON_EMPTY: Ty = Ty::Str { min: 1, max: None };
const NUM: Ty = Ty::Num {
    min: None,
    max: None,
    int: false,
    positive: false,
};
const ELEMENTS: Ty = Ty::Array {
    item: &Ty::Component,
    min: 0,
    max: Some(64),
};
const OPTION: Ty = Ty::Str {
    min: 1,
    max: Some(120),
};
const CHART_POINTS: Ty = Ty::Array {
    item: &Ty::Object(&[req("label", STR), req("value", NUM)]),
    min: 1,
    max: Some(100),
};

const fn req(name: &'static str, ty: Ty) -> Param {
    Param {
        name,
        required: true,
        ty,
    }
}

const fn opt(name: &'static str, ty: Ty) -> Param {
    Param {
        name,
        required: false,
        ty,
    }
}

const fn def(
    name: &'static str,
    description: &'static str,
    params: &'static [Param],
) -> ComponentDef {
    ComponentDef {
        name,
        description,
        advertised: true,
        field: false,
        params,
    }
}

const fn field(
    name: &'static str,
    description: &'static str,
    params: &'static [Param],
) -> ComponentDef {
    ComponentDef {
        name,
        description,
        advertised: true,
        field: true,
        params,
    }
}

pub(crate) static CATALOGUE: &[ComponentDef] = &[
    def(
        "Card",
        "Titled section that groups related content. Renders inline with the reply — no background — so keep nesting shallow.",
        &[opt("children", ELEMENTS), opt("title", STR)],
    ),
    def("Text", "Plain paragraph of body text.", &[req("text", STR)]),
    def(
        "Stat",
        "Single prominent metric — large value with a small label and optional hint.",
        &[req("label", STR), req("value", STR), opt("hint", STR)],
    ),
    def(
        "KeyValue",
        "Compact two-column list of key/value pairs — ids, branches, statuses.",
        &[req(
            "pairs",
            Ty::Array {
                item: &Ty::Object(&[req("key", STR), req("value", STR)]),
                min: 0,
                max: Some(100),
            },
        )],
    ),
    def(
        "Table",
        "Dense data table. columns is the header row; rows is one string array per row.",
        &[
            req(
                "columns",
                Ty::Array {
                    item: &STR,
                    min: 1,
                    max: Some(20),
                },
            ),
            req(
                "rows",
                Ty::Array {
                    item: &Ty::Array {
                        item: &STR,
                        min: 0,
                        max: Some(20),
                    },
                    min: 0,
                    max: Some(200),
                },
            ),
        ],
    ),
    def(
        "Progress",
        "Labeled horizontal progress bar; percent is 0-100.",
        &[
            req("label", STR),
            req(
                "percent",
                Ty::Num {
                    min: Some(0.0),
                    max: Some(100.0),
                    int: false,
                    positive: false,
                },
            ),
        ],
    ),
    def(
        "Badge",
        "Small status pill. tone is optional.",
        &[req("label", STR), opt("tone", Ty::Enum(TONES))],
    ),
    def(
        "List",
        "Simple bullet list of short strings.",
        &[req(
            "items",
            Ty::Array {
                item: &STR,
                min: 0,
                max: Some(100),
            },
        )],
    ),
    def(
        "Grid",
        "Responsive grid of components, collapsing to one column in narrow chats. columns is 1-4 (default 2).",
        &[
            req("children", ELEMENTS),
            opt(
                "columns",
                Ty::Num {
                    min: Some(1.0),
                    max: Some(4.0),
                    int: true,
                    positive: false,
                },
            ),
        ],
    ),
    def(
        "BarChart",
        "Horizontal comparison chart with visible labels and values. Supports negative values. Up to 100 points.",
        &[
            req("title", STR),
            req("points", CHART_POINTS),
            opt("unit", STR),
        ],
    ),
    def(
        "LineChart",
        "Ordered trend chart with an accessible data table. Points stay in supplied order; up to 100 points.",
        &[
            req("title", STR),
            req("points", CHART_POINTS),
            opt("unit", STR),
        ],
    ),
    def(
        "DonutChart",
        "Part-to-whole chart with a total and a labeled legend showing values and percentages. Use 1-12 non-negative points with a finite total; use BarChart to compare close values.",
        &[
            req("title", STR),
            req(
                "points",
                Ty::Array {
                    item: &Ty::Object(&[
                        req("label", STR),
                        req(
                            "value",
                            Ty::Num {
                                min: Some(0.0),
                                max: None,
                                int: false,
                                positive: false,
                            },
                        ),
                    ]),
                    min: 1,
                    max: Some(12),
                },
            ),
            opt("unit", STR),
        ],
    ),
    def(
        "Callout",
        "Highlighted note for a warning, risk, or key takeaway. tone defaults to neutral.",
        &[
            req("text", STR),
            opt("tone", Ty::Enum(TONES)),
            opt("title", STR),
        ],
    ),
    def(
        "Steps",
        "Ordered checklist or plan with a status per step: done, active, pending, or failed.",
        &[req(
            "items",
            Ty::Array {
                item: &Ty::Object(&[
                    req("label", STR),
                    req("status", Ty::Enum(STEP_STATUSES)),
                    opt("detail", STR),
                ]),
                min: 1,
                max: Some(20),
            },
        )],
    ),
    def(
        "Flow",
        "Diagram of 2-8 boxes joined by arrows in order — architecture, data flow, pipelines, handoffs. Mark the focal box with highlight. Runs left to right, stacking vertically in narrow chats.",
        &[
            req(
                "nodes",
                Ty::Array {
                    item: &Ty::Object(&[
                        req(
                            "label",
                            Ty::Str {
                                min: 1,
                                max: Some(60),
                            },
                        ),
                        opt(
                            "detail",
                            Ty::Str {
                                min: 0,
                                max: Some(200),
                            },
                        ),
                        opt("highlight", Ty::Bool),
                    ]),
                    min: 2,
                    max: Some(8),
                },
            ),
            opt(
                "caption",
                Ty::Str {
                    min: 0,
                    max: Some(200),
                },
            ),
        ],
    ),
    def(
        "Tabs",
        "Switchable views of alternatives, e.g. options or before/after. Each tab holds one component.",
        &[req(
            "tabs",
            Ty::Array {
                item: &Ty::Object(&[
                    req(
                        "label",
                        Ty::Str {
                            min: 1,
                            max: Some(40),
                        },
                    ),
                    req("content", Ty::Component),
                ]),
                min: 2,
                max: Some(6),
            },
        )],
    ),
    ComponentDef {
        name: "FollowUps",
        description: "Suggested next steps as buttons. Clicking one sends its prompt as the user's next message. 1-6 items.",
        advertised: false,
        field: false,
        params: &[req(
            "items",
            Ty::Array {
                item: &Ty::Object(&[
                    req(
                        "label",
                        Ty::Str {
                            min: 1,
                            max: Some(80),
                        },
                    ),
                    req(
                        "prompt",
                        Ty::Str {
                            min: 1,
                            max: Some(2000),
                        },
                    ),
                ]),
                min: 1,
                max: Some(6),
            },
        )],
    },
    def(
        "Form",
        "Collects structured input. Holds fields (and optional display components); submitting sends the answers as the user's next message.",
        &[
            req(
                "children",
                Ty::Array {
                    item: &Ty::Component,
                    min: 1,
                    max: Some(64),
                },
            ),
            opt(
                "submitLabel",
                Ty::Str {
                    min: 1,
                    max: Some(40),
                },
            ),
            opt("title", STR),
        ],
    ),
    field(
        "Input",
        "Single-line text field inside a Form.",
        &[
            req("name", Ty::FieldName),
            req("label", NON_EMPTY),
            opt("placeholder", STR),
            opt(
                "value",
                Ty::Str {
                    min: 0,
                    max: Some(2000),
                },
            ),
        ],
    ),
    field(
        "TextArea",
        "Multi-line text field inside a Form.",
        &[
            req("name", Ty::FieldName),
            req("label", NON_EMPTY),
            opt("placeholder", STR),
            opt(
                "value",
                Ty::Str {
                    min: 0,
                    max: Some(8000),
                },
            ),
        ],
    ),
    field(
        "Select",
        "Dropdown choice inside a Form; value preselects an option.",
        &[
            req("name", Ty::FieldName),
            req("label", NON_EMPTY),
            req(
                "options",
                Ty::Array {
                    item: &OPTION,
                    min: 1,
                    max: Some(50),
                },
            ),
            opt("value", STR),
        ],
    ),
    field(
        "RadioGroup",
        "Pick exactly one of 2-8 visible options inside a Form; value preselects one.",
        &[
            req("name", Ty::FieldName),
            req("label", NON_EMPTY),
            req(
                "options",
                Ty::Array {
                    item: &OPTION,
                    min: 2,
                    max: Some(8),
                },
            ),
            opt("value", STR),
        ],
    ),
    field(
        "Checkbox",
        "Yes/no toggle inside a Form.",
        &[
            req("name", Ty::FieldName),
            req("label", NON_EMPTY),
            opt("checked", Ty::Bool),
        ],
    ),
    field(
        "Slider",
        "Numeric range inside a Form; value defaults to min.",
        &[
            req("name", Ty::FieldName),
            req("label", NON_EMPTY),
            req("min", NUM),
            req("max", NUM),
            opt("value", NUM),
            opt(
                "step",
                Ty::Num {
                    min: None,
                    max: None,
                    int: false,
                    positive: true,
                },
            ),
        ],
    ),
];

pub(crate) fn lookup(name: &str) -> Option<&'static ComponentDef> {
    CATALOGUE.iter().find(|def| def.name == name)
}

/// Names agents may use, in catalogue order.
pub(crate) fn advertised_names() -> impl Iterator<Item = &'static str> {
    CATALOGUE
        .iter()
        .filter(|def| def.advertised)
        .map(|def| def.name)
}

impl Ty {
    /// Type label used in prompt signatures and type-mismatch errors.
    pub(crate) fn label(&self) -> String {
        match self {
            Ty::Str { .. } | Ty::FieldName => "string".into(),
            Ty::Num { .. } => "number".into(),
            Ty::Bool => "boolean".into(),
            Ty::Enum(values) => values
                .iter()
                .map(|value| format!("\"{value}\""))
                .collect::<Vec<_>>()
                .join(" | "),
            Ty::Array { item, .. } => format!("{}[]", item.label()),
            Ty::Object(fields) => format!(
                "{{ {} }}",
                fields
                    .iter()
                    .map(|field| format!("{}: {}", field.name, field.ty.label()))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Ty::Component => "any".into(),
        }
    }
}

impl ComponentDef {
    /// `Name(prop: type, prop?: type)`.
    pub(crate) fn signature(&self) -> String {
        let params = self
            .params
            .iter()
            .map(|param| {
                format!(
                    "{}{}: {}",
                    param.name,
                    if param.required { "" } else { "?" },
                    param.ty.label()
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!("{}({params})", self.name)
    }
}

/// One `Signature — description` line per advertised component, for prompts.
pub fn component_lines() -> String {
    CATALOGUE
        .iter()
        .filter(|def| def.advertised)
        .map(|def| format!("{} — {}", def.signature(), def.description))
        .collect::<Vec<_>>()
        .join("\n")
}
