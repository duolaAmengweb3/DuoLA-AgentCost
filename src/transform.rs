use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, serde::Serialize)]
pub struct RuleDefinition {
    pub id: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub safety: &'static str,
    pub default_enabled: bool,
    pub applies_to: &'static str,
}

/// The rule registry is deliberately static and inspectable.  A receipt only
/// stores a rule id; this catalogue lets the dashboard explain what that id
/// means without executing an LLM or trusting mutable prose in a request.
pub fn rule_registry() -> Vec<RuleDefinition> {
    vec![
        RuleDefinition {
            id: "tool-result.ansi.v1",
            title: "清理终端控制符",
            description: "移除颜色和光标控制码，不改变工具文本内容。",
            safety: "lossless-text",
            default_enabled: true,
            applies_to: "tool result text",
        },
        RuleDefinition {
            id: "tool-result.json-compact.v1",
            title: "紧凑化等价 JSON",
            description: "只去掉 JSON 空白，字段、类型和数组顺序保持不变。",
            safety: "structure-preserving",
            default_enabled: true,
            applies_to: "tool result text containing JSON",
        },
        RuleDefinition {
            id: "tool-result.repeated-lines.v1",
            title: "折叠连续重复日志",
            description: "保留首行和重复次数，不改错误码、尾部结果或调用身份。",
            safety: "lossy-with-receipt",
            default_enabled: true,
            applies_to: "tool result logs",
        },
        RuleDefinition {
            id: "tool-result.cap.v1",
            title: "显式工具输出上限",
            description: "只有用户明确配置上限时才保留首尾并标记中间省略。",
            safety: "explicit-opt-in",
            default_enabled: false,
            applies_to: "tool result text",
        },
        RuleDefinition {
            id: "tool-result.duplicate.v1",
            title: "同请求重复结果引用",
            description: "仅当同一请求已经出现完全相同结果且引用更短时替换。",
            safety: "hash-backed-reference",
            default_enabled: true,
            applies_to: "duplicate tool result text",
        },
        RuleDefinition {
            id: "tool-surface.dedupe.v1",
            title: "去重工具定义",
            description: "删除完全重复的工具定义，不改变任何工具 schema。",
            safety: "schema-preserving",
            default_enabled: true,
            applies_to: "tool definitions",
        },
        RuleDefinition {
            id: "tool-surface.relevance.v1",
            title: "任务相关工具面",
            description: "任务明确且工具很多时保留核心、已调用和名称/描述匹配的工具。",
            safety: "conservative-selection",
            default_enabled: true,
            applies_to: "large tool surfaces",
        },
        RuleDefinition {
            id: "routing.model-map.v1",
            title: "显式模型映射",
            description: "仅执行用户配置的 incoming model 到 upstream model 映射。",
            safety: "explicit-routing",
            default_enabled: false,
            applies_to: "request model field",
        },
        RuleDefinition {
            id: "budget.output-cap.v1",
            title: "输出 Token 上限",
            description: "仅按用户预算配置写入协议对应的最大输出字段。",
            safety: "explicit-budget",
            default_enabled: false,
            applies_to: "OpenAI/Anthropic model requests",
        },
    ]
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TransformPolicy {
    /// Remove terminal colour/control sequences from tool output.
    #[serde(default = "default_true")]
    pub strip_ansi: bool,
    /// Collapse three or more adjacent identical lines in tool output.
    #[serde(default = "default_true")]
    pub collapse_repeated_tool_lines: bool,
    /// Optional explicit cap for a tool result. Disabled by default because a
    /// cap can discard context and must be a deliberate user choice.
    #[serde(default)]
    pub max_tool_result_bytes: Option<usize>,
    /// Compact JSON strings returned by tools when the compact form is shorter.
    #[serde(default = "default_true")]
    pub compact_tool_json: bool,
    /// Replace an exact repeated tool result with a hash-backed reference when
    /// the original result is already present earlier in the same request.
    #[serde(default = "default_true")]
    pub dedupe_repeated_tool_results: bool,
    /// Remove duplicate tool definitions and, only when a request has a clear
    /// task and a large tool surface, keep relevant tools.
    #[serde(default = "default_true")]
    pub tool_surface_reduction: bool,
    #[serde(default = "default_tool_surface_min_tools")]
    pub tool_surface_min_tools: usize,
    #[serde(default = "default_tool_surface_min_keep")]
    pub tool_surface_min_keep: usize,
}

fn default_true() -> bool {
    true
}

fn default_tool_surface_min_tools() -> usize {
    16
}

fn default_tool_surface_min_keep() -> usize {
    4
}

impl Default for TransformPolicy {
    fn default() -> Self {
        Self {
            strip_ansi: true,
            collapse_repeated_tool_lines: true,
            max_tool_result_bytes: None,
            compact_tool_json: true,
            dedupe_repeated_tool_results: true,
            tool_surface_reduction: true,
            tool_surface_min_tools: default_tool_surface_min_tools(),
            tool_surface_min_keep: default_tool_surface_min_keep(),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Receipt {
    pub path: String,
    pub rule_id: String,
    pub original_hash: String,
    pub result_hash: String,
    pub original_bytes: usize,
    pub result_bytes: usize,
    pub status: String,
}

#[derive(Debug, Default)]
pub struct TransformResult {
    pub body: Vec<u8>,
    pub receipts: Vec<Receipt>,
    pub changed: bool,
    pub reason: Option<String>,
}

#[derive(Default)]
struct TransformContext {
    seen_tool_results: HashMap<String, String>,
}

fn hash(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    hex::encode(h.finalize())
}

/// Build a comparison view which removes only fields that the transform
/// engine is explicitly allowed to shorten. All routing, tool-call identity,
/// arguments, roles, message order and unknown fields remain in the view.
/// This is the last safety gate before an optimized body is sent upstream.
fn semantic_projection(value: &Value, mutable_tool_payload: bool) -> Value {
    match value {
        Value::Object(map) => {
            let is_tool_payload = mutable_tool_payload
                || map.get("role").and_then(Value::as_str) == Some("tool")
                || matches!(
                    map.get("type").and_then(Value::as_str),
                    Some("tool_result" | "function_call_output")
                );
            let mut projected = serde_json::Map::new();
            for (key, child) in map {
                // These fields are changed by an explicit, separately receipted
                // policy. Their semantic identity is checked elsewhere.
                if key == "tools"
                    || key == "model"
                    || key == "max_tokens"
                    || key == "max_completion_tokens"
                    || key == "max_output_tokens"
                {
                    continue;
                }
                if is_tool_payload && matches!(key.as_str(), "content" | "output") {
                    continue;
                }
                projected.insert(key.clone(), semantic_projection(child, is_tool_payload));
            }
            Value::Object(projected)
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| semantic_projection(item, mutable_tool_payload))
                .collect(),
        ),
        _ => value.clone(),
    }
}

fn collect_tool_call_ids(value: &Value, output: &mut HashSet<String>) {
    match value {
        Value::Object(map) => {
            for key in ["tool_call_id", "call_id", "id"] {
                if let Some(Value::String(id)) = map.get(key)
                    && (key != "id" || map.contains_key("type") || map.contains_key("tool_calls"))
                {
                    output.insert(id.clone());
                }
            }
            for child in map.values() {
                collect_tool_call_ids(child, output);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_tool_call_ids(item, output);
            }
        }
        _ => {}
    }
}

fn transform_preserves_semantics(original: &Value, transformed: &Value) -> bool {
    if semantic_projection(original, false) != semantic_projection(transformed, false) {
        return false;
    }
    let mut original_names = HashSet::new();
    let mut transformed_names = HashSet::new();
    collect_called_tool_names(original, &mut original_names);
    collect_called_tool_names(transformed, &mut transformed_names);
    if !original_names.is_subset(&transformed_names) {
        return false;
    }
    let mut original_ids = HashSet::new();
    let mut transformed_ids = HashSet::new();
    collect_tool_call_ids(original, &mut original_ids);
    collect_tool_call_ids(transformed, &mut transformed_ids);
    original_ids.is_subset(&transformed_ids)
}

fn strip_ansi(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0x1b {
            i += 1;
            if i < bytes.len() && bytes[i] == b'[' {
                i += 1;
                while i < bytes.len() {
                    let c = bytes[i];
                    i += 1;
                    if (b'@'..=b'~').contains(&c) {
                        break;
                    }
                }
            } else if i < bytes.len() {
                i += 1;
            }
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| input.to_owned())
}

fn collapse_repeated_lines(input: &str) -> String {
    let lines: Vec<&str> = input.split_inclusive('\n').collect();
    if lines.len() < 3 {
        return input.to_owned();
    }
    let mut out = String::with_capacity(input.len());
    let mut index = 0;
    let mut changed = false;
    while index < lines.len() {
        let current = lines[index];
        let current_without_newline = current.strip_suffix('\n').unwrap_or(current);
        let mut end = index + 1;
        while end < lines.len()
            && lines[end].strip_suffix('\n').unwrap_or(lines[end]) == current_without_newline
        {
            end += 1;
        }
        let count = end - index;
        if count >= 3 {
            out.push_str(current);
            let suffix = if current.ends_with('\n') { "\n" } else { "" };
            out.push_str(&format!(
                "[DuoLA] previous tool line repeated {} more times{}",
                count - 1,
                suffix
            ));
            changed = true;
        } else {
            out.push_str(&lines[index..end].concat());
        }
        index = end;
    }
    if changed && out.len() < input.len() {
        out
    } else {
        input.to_owned()
    }
}

fn compact_json(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if !(trimmed.starts_with('{') || trimmed.starts_with('[')) {
        return None;
    }
    let value = serde_json::from_str::<Value>(trimmed).ok()?;
    let compact = serde_json::to_string(&value).ok()?;
    (compact.len() < input.len()).then_some(compact)
}

fn cap_tool_result(input: &str, max_bytes: usize) -> String {
    if max_bytes == 0 || input.len() <= max_bytes {
        return input.to_owned();
    }
    let marker = format!(
        "\n[DuoLA] tool output capped at {} bytes; middle omitted.\n",
        max_bytes
    );
    if marker.len() >= max_bytes {
        return marker[..marker.floor_char_boundary(max_bytes)].to_owned();
    }
    let remaining = max_bytes - marker.len();
    let head_budget = remaining / 2;
    let tail_budget = remaining - head_budget;
    let head_end = input.floor_char_boundary(head_budget.min(input.len()));
    let tail_start = input.len().saturating_sub(tail_budget);
    let tail_start = input.ceil_char_boundary(tail_start);
    format!("{}{}{}", &input[..head_end], marker, &input[tail_start..])
}

fn record_change(
    current: &mut String,
    next: String,
    path: &str,
    rule_id: &str,
    receipts: &mut Vec<Receipt>,
) {
    if next == *current || next.len() >= current.len() {
        return;
    }
    receipts.push(Receipt {
        path: path.to_owned(),
        rule_id: rule_id.to_owned(),
        original_hash: hash(current.as_bytes()),
        result_hash: hash(next.as_bytes()),
        original_bytes: current.len(),
        result_bytes: next.len(),
        status: "applied".into(),
    });
    *current = next;
}

fn transform_value(
    value: &mut Value,
    path: &str,
    context: &mut TransformContext,
    receipts: &mut Vec<Receipt>,
    policy: &TransformPolicy,
    inherited_tool_payload: bool,
) {
    match value {
        Value::Object(map) => {
            let tool_role = map
                .get("role")
                .and_then(Value::as_str)
                .is_some_and(|v| v == "tool");
            let block_type = map
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let function_output = block_type == "function_call_output";
            let tool_result = block_type == "tool_result" || map.contains_key("tool_result");
            let is_tool_payload =
                inherited_tool_payload || tool_role || function_output || tool_result;
            for (key, child) in map.iter_mut() {
                let child_path = format!("{path}/{key}");
                let safe_string =
                    matches!(key.as_str(), "output" | "content" | "text") && is_tool_payload;
                if safe_string && let Value::String(text) = child {
                    let mut cleaned = text.clone();
                    if policy.strip_ansi {
                        let next = strip_ansi(&cleaned);
                        record_change(
                            &mut cleaned,
                            next,
                            &child_path,
                            "tool-result.ansi.v1",
                            receipts,
                        );
                    }
                    if policy.compact_tool_json
                        && let Some(next) = compact_json(&cleaned)
                    {
                        record_change(
                            &mut cleaned,
                            next,
                            &child_path,
                            "tool-result.json-compact.v1",
                            receipts,
                        );
                    }
                    if policy.collapse_repeated_tool_lines {
                        let next = collapse_repeated_lines(&cleaned);
                        record_change(
                            &mut cleaned,
                            next,
                            &child_path,
                            "tool-result.repeated-lines.v1",
                            receipts,
                        );
                    }
                    if let Some(max_bytes) = policy.max_tool_result_bytes {
                        let next = cap_tool_result(&cleaned, max_bytes);
                        record_change(
                            &mut cleaned,
                            next,
                            &child_path,
                            "tool-result.cap.v1",
                            receipts,
                        );
                    }
                    if policy.dedupe_repeated_tool_results {
                        let result_hash = hash(cleaned.as_bytes());
                        if let Some(first_path) = context.seen_tool_results.get(&result_hash) {
                            let marker = format!(
                                "[DuoLA] duplicate tool result; ref={first_path}; hash={}",
                                &result_hash[..12]
                            );
                            record_change(
                                &mut cleaned,
                                marker,
                                &child_path,
                                "tool-result.duplicate.v1",
                                receipts,
                            );
                        } else {
                            context
                                .seen_tool_results
                                .insert(result_hash, child_path.clone());
                        }
                    }
                    if cleaned != *text {
                        *text = cleaned;
                    }
                }
                transform_value(
                    child,
                    &child_path,
                    context,
                    receipts,
                    policy,
                    is_tool_payload,
                );
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter_mut().enumerate() {
                transform_value(
                    child,
                    &format!("{path}/{index}"),
                    context,
                    receipts,
                    policy,
                    inherited_tool_payload,
                );
            }
        }
        _ => {}
    }
}

fn collect_strings(value: &Value, output: &mut String) {
    match value {
        Value::String(text) => {
            output.push_str(text);
            output.push('\n');
        }
        Value::Object(map) => {
            for child in map.values() {
                collect_strings(child, output);
            }
        }
        Value::Array(items) => {
            for child in items {
                collect_strings(child, output);
            }
        }
        _ => {}
    }
}

fn collect_user_text(value: &Value, output: &mut String) {
    match value {
        Value::Object(map) => {
            if map.get("role").and_then(Value::as_str) == Some("user") {
                for key in ["content", "input", "text"] {
                    if let Some(child) = map.get(key) {
                        collect_strings(child, output);
                    }
                }
            } else if map.contains_key("input")
                && !map.contains_key("role")
                && let Some(child) = map.get("input")
            {
                collect_strings(child, output);
            }
            for child in map.values() {
                collect_user_text(child, output);
            }
        }
        Value::Array(items) => {
            for child in items {
                collect_user_text(child, output);
            }
        }
        _ => {}
    }
}

fn tokens(text: &str) -> HashSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|token| token.chars().count() >= 3)
        .map(|token| token.to_ascii_lowercase())
        .collect()
}

fn collect_call_names(value: &Value, output: &mut HashSet<String>) {
    match value {
        Value::Object(map) => {
            if let Some(Value::Object(function)) = map.get("function")
                && let Some(Value::String(name)) = function.get("name")
            {
                output.insert(name.to_ascii_lowercase());
            }
            if map.get("type").and_then(Value::as_str) == Some("function_call")
                && let Some(Value::String(name)) = map.get("name")
            {
                output.insert(name.to_ascii_lowercase());
            }
            for child in map.values() {
                collect_call_names(child, output);
            }
        }
        Value::Array(items) => {
            for child in items {
                collect_call_names(child, output);
            }
        }
        _ => {}
    }
}

fn collect_called_tool_names(value: &Value, output: &mut HashSet<String>) {
    match value {
        Value::Object(map) => {
            if let Some(child) = map.get("tool_calls") {
                collect_call_names(child, output);
            }
            if let Some(child) = map.get("function_call") {
                collect_call_names(child, output);
            }
            if let Some(child) = map.get("tool_choice") {
                collect_call_names(child, output);
            }
            if map.get("type").and_then(Value::as_str) == Some("function_call") {
                collect_call_names(value, output);
            }
            for (key, child) in map {
                if key != "tools" && key != "function" && key != "parameters" {
                    collect_called_tool_names(child, output);
                }
            }
        }
        Value::Array(items) => {
            for child in items {
                collect_called_tool_names(child, output);
            }
        }
        _ => {}
    }
}

fn tool_name_and_description(tool: &Value) -> (String, String) {
    let mut name = String::new();
    let mut description = String::new();
    if let Value::Object(map) = tool {
        if let Some(Value::String(value)) = map.get("name") {
            name = value.clone();
        }
        if let Some(Value::String(value)) = map.get("description") {
            description = value.clone();
        }
        if let Some(Value::Object(function)) = map.get("function") {
            if let Some(Value::String(value)) = function.get("name") {
                name = value.clone();
            }
            if let Some(Value::String(value)) = function.get("description") {
                description = value.clone();
            }
        }
    }
    (name, description)
}

fn is_core_agent_tool(name: &str, description: &str) -> bool {
    let text = format!(
        "{} {}",
        name.to_ascii_lowercase(),
        description.to_ascii_lowercase()
    );
    [
        "read",
        "write",
        "edit",
        "shell",
        "terminal",
        "execute",
        "run",
        "search",
        "grep",
        "git",
        "file",
        "repository",
        "code",
    ]
    .iter()
    .any(|token| {
        text.split(|c: char| !c.is_alphanumeric())
            .any(|part| part == *token)
    })
}

fn is_side_effect_tool(name: &str, description: &str) -> bool {
    let text = format!(
        "{} {}",
        name.to_ascii_lowercase(),
        description.to_ascii_lowercase()
    );
    [
        "send", "post", "put", "patch", "delete", "write", "edit", "create", "update", "publish",
        "deploy", "execute", "transfer", "trade", "buy", "sell", "charge", "email", "message",
        "invite", "commit", "merge", "database", "payment", "withdraw",
    ]
    .iter()
    .any(|token| {
        text.split(|c: char| !c.is_alphanumeric())
            .any(|part| part == *token)
    })
}

fn transform_tool_surface(
    value: &mut Value,
    path: &str,
    policy: &TransformPolicy,
    receipts: &mut Vec<Receipt>,
    query_tokens: &HashSet<String>,
    called_names: &HashSet<String>,
) {
    match value {
        Value::Object(map) => {
            for (key, child) in map.iter_mut() {
                let child_path = format!("{path}/{key}");
                if key == "tools"
                    && policy.tool_surface_reduction
                    && let Value::Array(items) = child
                    && items.len() >= 2
                {
                    let original = serde_json::to_vec(items).unwrap_or_default();
                    let mut seen = HashSet::new();
                    let mut selected = Vec::with_capacity(items.len());
                    for item in items.iter() {
                        let fingerprint = serde_json::to_string(item).unwrap_or_default();
                        if seen.insert(fingerprint) {
                            selected.push(item.clone());
                        }
                    }
                    let has_side_effect_tool = selected.iter().any(|item| {
                        let (name, description) = tool_name_and_description(item);
                        is_side_effect_tool(&name, &description)
                    });
                    if selected.len() >= policy.tool_surface_min_tools
                        && !query_tokens.is_empty()
                        && !has_side_effect_tool
                    {
                        let mut relevant = Vec::new();
                        for (index, item) in selected.iter().enumerate() {
                            let (name, description) = tool_name_and_description(item);
                            let name_lower = name.to_ascii_lowercase();
                            let overlap = tokens(&name).intersection(query_tokens).count()
                                + tokens(&description).intersection(query_tokens).count();
                            let unknown = name.is_empty() || description.is_empty();
                            if unknown
                                || is_core_agent_tool(&name, &description)
                                || overlap > 0
                                || called_names.contains(&name_lower)
                            {
                                relevant.push(index);
                            }
                        }
                        if relevant.len() >= policy.tool_surface_min_keep
                            && relevant.len() < selected.len()
                        {
                            selected = relevant
                                .into_iter()
                                .map(|index| selected[index].clone())
                                .collect();
                        }
                    }
                    let result = serde_json::to_vec(&selected).unwrap_or_default();
                    if result.len() < original.len() {
                        let rule_id = if selected.len() < items.len() {
                            "tool-surface.relevance.v1"
                        } else {
                            "tool-surface.dedupe.v1"
                        };
                        receipts.push(Receipt {
                            path: child_path.clone(),
                            rule_id: rule_id.into(),
                            original_hash: hash(&original),
                            result_hash: hash(&result),
                            original_bytes: original.len(),
                            result_bytes: result.len(),
                            status: "applied".into(),
                        });
                        *child = Value::Array(selected);
                    }
                }
                transform_tool_surface(
                    child,
                    &child_path,
                    policy,
                    receipts,
                    query_tokens,
                    called_names,
                );
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter_mut().enumerate() {
                transform_tool_surface(
                    child,
                    &format!("{path}/{index}"),
                    policy,
                    receipts,
                    query_tokens,
                    called_names,
                );
            }
        }
        _ => {}
    }
}

pub fn transform_json(body: &[u8]) -> TransformResult {
    transform_json_with_policy(body, &TransformPolicy::default())
}

pub fn transform_json_with_policy(body: &[u8], policy: &TransformPolicy) -> TransformResult {
    let Ok(mut value) = serde_json::from_slice::<Value>(body) else {
        return TransformResult {
            body: body.to_vec(),
            reason: Some("请求不是可识别 JSON，原样透传".into()),
            ..Default::default()
        };
    };
    let original_value = value.clone();
    let mut receipts = vec![];
    let mut query = String::new();
    collect_user_text(&value, &mut query);
    let query_tokens = tokens(&query);
    let mut called_names = HashSet::new();
    collect_called_tool_names(&value, &mut called_names);
    transform_tool_surface(
        &mut value,
        "$",
        policy,
        &mut receipts,
        &query_tokens,
        &called_names,
    );
    transform_value(
        &mut value,
        "$",
        &mut TransformContext::default(),
        &mut receipts,
        policy,
        false,
    );
    if receipts.is_empty() {
        return TransformResult {
            body: body.to_vec(),
            receipts,
            changed: false,
            reason: Some("没有可安全且有正收益的确定性规则".into()),
        };
    }
    let out = serde_json::to_vec(&value).unwrap_or_else(|_| body.to_vec());
    if out.len() >= body.len() || !transform_preserves_semantics(&original_value, &value) {
        return TransformResult {
            body: body.to_vec(),
            receipts: vec![],
            changed: false,
            reason: Some(if out.len() >= body.len() {
                "处理结果没有变短".into()
            } else {
                "语义护栏未通过，原样透传".into()
            }),
        };
    }
    TransformResult {
        body: out,
        receipts,
        changed: true,
        reason: None,
    }
}

pub fn estimate_tokens(bytes: usize) -> i64 {
    ((bytes as f64) / 4.0).ceil() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_ansi_only_from_tool_output() {
        let input = br#"{"role":"tool","content":"ok \u001b[31mred\u001b[0m"}"#;
        let out = transform_json(input);
        assert!(out.changed);
        assert_eq!(out.receipts.len(), 1);
        let value: Value = serde_json::from_slice(&out.body).unwrap();
        assert_eq!(value["role"], "tool");
        assert_eq!(value["content"], "ok red");
    }

    #[test]
    fn transforms_structured_anthropic_tool_text_without_touching_call_identity() {
        let input = serde_json::json!({
            "model": "claude",
            "messages": [{
                "role": "user",
                "content": [{"type": "text", "text": "inspect logs"}]
            }, {
                "role": "user",
                "content": [{"type": "tool_result", "tool_use_id": "call_1", "content": [{"type": "text", "text": "\u{1b}[31merror\u{1b}[0m\nline\nline\nline\n"}]}]
            }]
        });
        let input = serde_json::to_vec(&input).unwrap();
        let out = transform_json(&input);
        assert!(out.changed);
        let value: Value = serde_json::from_slice(&out.body).unwrap();
        assert_eq!(value["messages"][1]["content"][0]["tool_use_id"], "call_1");
        assert_eq!(
            value["messages"][1]["content"][0]["content"][0]["type"],
            "text"
        );
        assert!(
            !value["messages"][1]["content"][0]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains('\u{1b}')
        );
    }

    #[test]
    fn preserves_user_text() {
        let input = br#"{"role":"user","content":"\u001b[31mkeep\u001b[0m"}"#;
        let out = transform_json(input);
        assert!(!out.changed);
        assert_eq!(out.body, input);
    }

    #[test]
    fn compacts_json_tool_output() {
        let input = br#"{"role":"tool","content":"{\n  \"ok\": true,\n  \"items\": [1, 2, 3]\n}"}"#;
        let out = transform_json(input);
        assert!(out.changed);
        assert!(
            out.receipts
                .iter()
                .any(|r| r.rule_id == "tool-result.json-compact.v1")
        );
        let value: Value = serde_json::from_slice(&out.body).unwrap();
        let compact: Value = serde_json::from_str(value["content"].as_str().unwrap()).unwrap();
        assert_eq!(compact["ok"], true);
        assert_eq!(compact["items"], serde_json::json!([1, 2, 3]));
    }

    #[test]
    fn collapses_long_repeated_tool_lines() {
        let line = "warning: dependency resolver retried request with identical payload 0123456789";
        let content = format!("{line}\n{line}\n{line}\n{line}\nresult\n");
        let input = serde_json::json!({"role":"tool","content":content});
        let input = serde_json::to_vec(&input).unwrap();
        let out = transform_json(&input);
        assert!(out.changed);
        assert!(out.body.len() < input.len());
        assert!(
            out.receipts
                .iter()
                .any(|r| r.rule_id == "tool-result.repeated-lines.v1")
        );
    }

    #[test]
    fn does_not_expand_short_repeated_lines() {
        let input = br#"{"role":"tool","content":"x\nx\nx\n"}"#;
        let out = transform_json(input);
        assert!(!out.changed);
        assert_eq!(out.body, input);
    }

    #[test]
    fn dedupes_repeated_tool_results_only_when_marker_is_shorter() {
        let content =
            "A long tool result that appears in the same request more than once. ".repeat(8);
        let input = serde_json::json!({
            "messages": [
                {"role":"tool","content":content},
                {"role":"tool","content":content}
            ]
        });
        let input = serde_json::to_vec(&input).unwrap();
        let out = transform_json(&input);
        assert!(out.changed);
        assert!(
            out.receipts
                .iter()
                .any(|r| r.rule_id == "tool-result.duplicate.v1")
        );
        assert!(out.body.len() < input.len());
    }

    #[test]
    fn reduces_large_tool_surface_using_user_task() {
        let mut tools = Vec::new();
        for index in 0..20 {
            tools.push(serde_json::json!({
                "type":"function",
                "function": {
                    "name": format!("tool_{index}"),
                    "description": if index < 5 { "search repository files" } else { "manage unrelated calendar events" },
                    "parameters": {"type":"object","properties":{"query":{"type":"string"}}}
                }
            }));
        }
        let input = serde_json::json!({
            "model":"test",
            "messages":[{"role":"user","content":"search repository files"}],
            "tools":tools
        });
        let input = serde_json::to_vec(&input).unwrap();
        let out = transform_json(&input);
        assert!(out.changed);
        assert!(
            out.receipts
                .iter()
                .any(|r| r.rule_id == "tool-surface.relevance.v1")
        );
        let value: Value = serde_json::from_slice(&out.body).unwrap();
        assert_eq!(value["tools"].as_array().unwrap().len(), 5);
    }

    #[test]
    fn keeps_small_or_ambiguous_tool_surface() {
        let input = serde_json::json!({
            "messages":[{"role":"user","content":"do the task"}],
            "tools":[
                {"type":"function","function":{"name":"one","description":"first"}},
                {"type":"function","function":{"name":"two","description":"second"}}
            ]
        });
        let input = serde_json::to_vec(&input).unwrap();
        let out = transform_json(&input);
        assert!(
            !out.receipts
                .iter()
                .any(|r| r.rule_id == "tool-surface.relevance.v1")
        );
    }

    #[test]
    fn records_each_applied_rule_in_order() {
        let line = "warning: dependency resolver retried request with identical payload 0123456789";
        let input = serde_json::json!({
            "role":"tool",
            "content":format!("\u{1b}[31m{line}\u{1b}[0m\n{line}\n{line}\n")
        });
        let input = serde_json::to_vec(&input).unwrap();
        let out = transform_json_with_policy(
            &input,
            &TransformPolicy {
                max_tool_result_bytes: Some(180),
                ..Default::default()
            },
        );
        assert!(out.changed);
        assert!(out.receipts.len() >= 2);
        for pair in out.receipts.windows(2) {
            if pair[0].path == pair[1].path {
                assert_eq!(pair[0].result_hash, pair[1].original_hash);
            }
        }
    }
}
