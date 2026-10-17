//! One field description generates model schemas and validates local execution.
use crate::{Error, ToolCategory, ToolDefinition, Tools};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::OnceLock};
#[derive(Clone)]
enum Shape {
    Text(usize, usize, Option<&'static str>),
    Number(u64, u64),
    Boolean,
    Nullable(Box<Shape>),
    Array(Box<Shape>, usize, usize),
    Choice(&'static [&'static str]),
    Object(Vec<Field>),
}
#[derive(Clone)]
struct Field {
    name: &'static str,
    shape: Shape,
    required: bool,
    default: Option<Value>,
}
impl Field {
    fn new(name: &'static str, shape: Shape) -> Self {
        Self {
            name,
            shape,
            required: true,
            default: None,
        }
    }
    fn optional(mut self) -> Self {
        self.required = false;
        self
    }
    fn default(mut self, value: Value) -> Self {
        self.required = false;
        self.default = Some(value);
        self
    }
}
fn error(field: &str) -> Error {
    Error::Tool(format!("Invalid native tool parameter: {field}"))
}
impl Shape {
    fn schema(&self) -> Value {
        match self {
            Self::Text(min, max, format) => {
                let mut value = json!({"type":"string","minLength":min,"maxLength":max});
                if let Some(format) = format {
                    if *format == "sha256" {
                        value["pattern"] = json!("^[a-f0-9]{64}$");
                    } else {
                        value["format"] = json!(format);
                    }
                }
                value
            }
            Self::Number(min, max) => json!({"type":"integer","minimum":min,"maximum":max}),
            Self::Boolean => json!({"type":"boolean"}),
            Self::Nullable(shape) => json!({"anyOf":[shape.schema(),{"type":"null"}]}),
            Self::Array(item, min, max) => {
                json!({"type":"array","items":item.schema(),"minItems":min,"maxItems":max})
            }
            Self::Choice(values) => json!({"type":"string","enum":values}),
            Self::Object(fields) => {
                let mut properties = serde_json::Map::new();
                let mut required = vec![];
                for field in fields {
                    let mut shape = field.shape.schema();
                    if let Some(default) = &field.default {
                        shape["default"] = default.clone();
                    }
                    properties.insert(field.name.into(), shape);
                    if field.required {
                        required.push(field.name);
                    }
                }
                json!({"type":"object","properties":properties,"required":required,"additionalProperties":false})
            }
        }
    }
    fn validate(&self, value: &mut Value, path: &str) -> Result<(), Error> {
        let valid = match self {
            Self::Text(min, max, format) => value.as_str().is_some_and(|text| {
                let count = text.chars().count();
                count >= *min
                    && count <= *max
                    && match format {
                        Some("sha256") => {
                            text.len() == 64
                                && text
                                    .bytes()
                                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                        }
                        Some("uuid") => uuid::Uuid::parse_str(text).is_ok(),
                        _ => true,
                    }
            }),
            Self::Number(min, max) => value.as_u64().is_some_and(|n| n >= *min && n <= *max),
            Self::Boolean => value.is_boolean(),
            Self::Choice(values) => value.as_str().is_some_and(|text| values.contains(&text)),
            Self::Nullable(shape) => {
                if !value.is_null() {
                    shape.validate(value, path)?;
                }
                true
            }
            Self::Array(shape, min, max) => {
                let values = value.as_array_mut().ok_or_else(|| error(path))?;
                if values.len() < *min || values.len() > *max {
                    return Err(error(path));
                }
                for (i, value) in values.iter_mut().enumerate() {
                    shape.validate(value, &format!("{path}[{i}]"))?;
                }
                true
            }
            Self::Object(fields) => {
                let values = value.as_object_mut().ok_or_else(|| error(path))?;
                if let Some(name) = values
                    .keys()
                    .find(|name| !fields.iter().any(|field| field.name == name.as_str()))
                {
                    return Err(error(&format!("{path}.{name}")));
                }
                for field in fields {
                    if !values.contains_key(field.name) {
                        if let Some(default) = &field.default {
                            values.insert(field.name.into(), default.clone());
                        } else if field.required {
                            return Err(error(&format!("{path}.{}", field.name)));
                        } else {
                            continue;
                        }
                    }
                    field.shape.validate(
                        values.get_mut(field.name).unwrap(),
                        &format!("{path}.{}", field.name),
                    )?;
                }
                true
            }
        };
        if valid { Ok(()) } else { Err(error(path)) }
    }
}
struct Specification {
    description: &'static str,
    category: ToolCategory,
    shape: Shape,
}
fn specifications() -> &'static BTreeMap<&'static str, Specification> {
    static SPECS: OnceLock<BTreeMap<&'static str, Specification>> = OnceLock::new();
    SPECS.get_or_init(||{
        use Shape::*;use ToolCategory::{Read,Edit,Execute};
        let path=||Text(1,4096,None);let text=||Text(0,2*1024*1024,None);let hash=||Text(64,64,Some("sha256"));let id=||Text(1,64,Some("uuid"));
        let field=Field::new;let dir=||field("path",path()).default(json!("."));let process=||field("process_id",id());
        let edit=||vec![field("old_string",Text(1,2*1024*1024,None)),field("new_string",text()),field("replace_all",Boolean).default(json!(false))];
        let mut specs=BTreeMap::new();
        let mut add=|name,category,description,fields|{specs.insert(name,Specification {description,category,shape:Object(fields)});};
        add("read_file",Read,"Read fresh UTF-8 lines and SHA-256. Offsets are zero-based; use the returned hash for edits.",vec![field("path",path()),field("offset",Number(0,u64::MAX)).default(json!(0)),field("limit",Number(1,2000)).default(json!(400))]);
        add("read_image",Read,"Read a checkout PNG, JPEG, GIF or WebP as image content.",vec![field("path",path())]);
        add("list_directory",Read,"List immediate directory entries including hidden names.",vec![dir()]);
        add("glob",Read,"Find files by ripgrep glob. Honors ignore rules and excludes symlink traversal.",vec![field("pattern",Text(1,512,None)),dir()]);
        add("grep",Read,"Search fresh checkout text with ripgrep, including file names and line numbers.",vec![field("pattern",Text(1,4096,None)),dir(),field("literal",Boolean).default(json!(false)),field("ignore_case",Boolean).default(json!(false)),field("glob",Text(0,512,None)).optional()]);
        add("write_file",Edit,"Create or replace UTF-8 text. Null expected_hash creates exclusively; replacement requires a fresh read_file hash.",vec![field("path",path()),field("content",text()),field("expected_hash",Nullable(Box::new(hash())))]);
        let mut fields=vec![field("path",path()),field("expected_hash",hash())];fields.extend(edit());
        add("edit_file",Edit,"Apply a literal replacement against a fresh file hash. Ambiguous matches require replace_all.",fields);
        add("multi_edit",Edit,"Apply ordered literal replacements to one file atomically. All edits must validate before any content is written.",vec![field("path",path()),field("expected_hash",hash()),field("edits",Array(Box::new(Object(edit())),1,100))]);
        add("exec_command",Execute,"Run supervised Bash in the checkout. Background output stays in an artifact; use process_id for stdin, output or waiting. Completion and optional literal readiness notices reach this conversation.",vec![field("command",Text(1,64000,None)),field("cwd",path()).default(json!(".")),field("label",Text(0,120,None)).optional(),field("yield_time_ms",Number(0,30000)).default(json!(1000)),field("timeout_ms",Number(100,86400000)).default(json!(1800000)),field("notify_on_output",Text(1,256,None)).optional()]);
        add("process_output",Read,"Read output from a byte cursor. Use next_cursor to avoid rereading; includes status and artifact location.",vec![process(),field("cursor",Number(0,u64::MAX)).default(json!(0)),field("max_bytes",Number(1,64000)).default(json!(16000))]);
        add("write_stdin",Execute,"Send input to a managed process. eof closes stdin; control characters are data, so use stop_process to terminate.",vec![process(),field("data",Text(0,64000,None)).default(json!("")),field("eof",Boolean).default(json!(false))]);
        add("wait_process",Read,"Wait up to 30 seconds, then return status and output without starting another command.",vec![process(),field("timeout_ms",Number(0,30000)).default(json!(1000))]);
        add("list_processes",Read,"List this agent's commands. The root may inspect its children's commands.",vec![]);
        add("stop_process",Execute,"Terminate a managed command and its owned process group.",vec![process()]);
        add("update_plan",Read,"Update the visible plan. At most one step may be in progress.",vec![field("steps",Array(Box::new(Object(vec![field("step",Text(1,500,None)),field("status",Choice(&["pending","in_progress","completed"]))])),0,30))]);
        add("request_user_input",Read,"Ask the user a question and wait for an answer.",vec![field("question",Text(1,2000,None)),field("options",Array(Box::new(Text(1,300,None)),2,5)).optional()]);
        specs
    })
}
pub(super) fn definitions(questions: bool, plans: bool) -> Tools {
    specifications()
        .iter()
        .filter(|(name, _)| {
            (**name != "request_user_input" || questions) && (**name != "update_plan" || plans)
        })
        .map(|(name, spec)| {
            (
                (*name).into(),
                ToolDefinition {
                    description: spec.description.into(),
                    category: spec.category,
                    parameters: spec.shape.schema(),
                },
            )
        })
        .collect()
}
pub(super) fn validate(name: &str, value: &mut Value) -> Result<(), Error> {
    specifications()
        .get(name)
        .ok_or_else(|| Error::Tool("Unknown native tool".into()))?
        .shape
        .validate(value, name)
}
