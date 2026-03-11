use anyhow::{Context, Result, anyhow};
use chrono::Utc;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use super::{
    AuthScheme, AuthSpec, BodySpec, ExpectedResponse, HeaderParam, PathParam, QueryParam,
    RequestAuth, RequestReplay, RequestReplayBundle, RequestVariant, ServiceMetadata,
    VariantRequest, openapi,
};

const DEFAULT_LOCAL_URL: &str = "http://localhost:9100";

pub async fn generate(
    output_dir: &str,
    overwrite: bool,
    include_private: bool,
    include_full: bool,
) -> Result<()> {
    let output_dir = PathBuf::from(output_dir);

    ensure_dir(&output_dir)?;
    ensure_dir(&output_dir.join("requests"))?;
    ensure_dir(&output_dir.join("schemas"))?;

    let discovery = openapi::discover_or_generate(&output_dir).await.map_err(|e| {
        anyhow!(
            "{e}\n\nHint: set PRISM_OPENAPI_PATH or PRISM_OPENAPI_URL, or install swag for Go projects."
        )
    })?;
    let spec = openapi::read_openapi_value(&discovery.spec_path)?;

    let (service_name, service_description) = extract_service_info(&spec);
    let openapi_source = discovery.source;

    let auth_spec = extract_auth_spec(&spec);
    let base_urls = build_base_urls();

    let mut schema_map = write_component_schemas(&spec, &output_dir, overwrite)?;
    let requests = build_requests(
        &spec,
        &output_dir,
        overwrite,
        include_private,
        include_full,
        &mut schema_map,
    )?;

    let bundle = RequestReplayBundle {
        version: "0.1".to_string(),
        service: ServiceMetadata {
            name: service_name,
            description: service_description,
            openapi_source,
            generated_at: Utc::now().to_rfc3339(),
        },
        auth: auth_spec,
        base_urls,
        requests,
    };

    let index_path = output_dir.join("index.json");
    write_json(&index_path, &bundle, overwrite)?;

    Ok(())
}

fn extract_service_info(spec: &Value) -> (String, Option<String>) {
    let info = spec.get("info").unwrap_or(&Value::Null);
    let title = info
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("Service")
        .to_string();
    let description = info
        .get("description")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    (title, description)
}

fn build_base_urls() -> BTreeMap<String, String> {
    let mut base_urls = BTreeMap::new();
    let local = std::env::var("PRISM_LOCAL_URL").unwrap_or_else(|_| DEFAULT_LOCAL_URL.to_string());
    base_urls.insert("local".to_string(), local);

    if let Ok(dev) = std::env::var("PRISM_DEV_URL") {
        base_urls.insert("dev".to_string(), dev);
    }
    if let Ok(staging) = std::env::var("PRISM_STAGING_URL") {
        base_urls.insert("staging".to_string(), staging);
    }

    base_urls
}

fn extract_auth_spec(spec: &Value) -> AuthSpec {
    let mut schemes = Vec::new();
    if let Some(schemes_val) = spec
        .get("components")
        .and_then(|c| c.get("securitySchemes"))
        .and_then(|v| v.as_object())
    {
        for (name, scheme) in schemes_val {
            let scheme_type = scheme.get("type").and_then(|v| v.as_str()).unwrap_or("");
            let header = scheme
                .get("name")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let prefix = if scheme_type == "http"
                && scheme.get("scheme").and_then(|v| v.as_str()) == Some("bearer")
            {
                Some("Bearer".to_string())
            } else {
                None
            };
            let env_var = Some("PRISM_API_KEY".to_string());
            let description = scheme
                .get("description")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            schemes.push(AuthScheme {
                id: name.clone(),
                r#type: scheme_type.to_string(),
                header,
                prefix,
                env_var,
                description,
            });
        }
    }

    if schemes.is_empty() {
        schemes.push(AuthScheme {
            id: "bearerAuth".to_string(),
            r#type: "http".to_string(),
            header: Some("Authorization".to_string()),
            prefix: Some("Bearer".to_string()),
            env_var: Some("PRISM_API_KEY".to_string()),
            description: Some("Default bearer auth".to_string()),
        });
    }

    let default = schemes.first().map(|s| s.id.clone());
    AuthSpec { schemes, default }
}

fn write_component_schemas(
    spec: &Value,
    output_dir: &Path,
    overwrite: bool,
) -> Result<HashMap<String, String>> {
    let mut schema_map = HashMap::new();
    let schemas = spec
        .get("components")
        .and_then(|c| c.get("schemas"))
        .and_then(|v| v.as_object());

    if let Some(schemas) = schemas {
        for (name, schema) in schemas {
            let file_name = format!("{name}.json");
            let path = output_dir.join("schemas").join(&file_name);
            if write_json(&path, schema, overwrite)? || path.exists() {
                schema_map.insert(name.clone(), format!("schemas/{file_name}"));
            }
        }
    }

    Ok(schema_map)
}

fn build_requests(
    spec: &Value,
    output_dir: &Path,
    overwrite: bool,
    include_private: bool,
    include_full: bool,
    schema_map: &mut HashMap<String, String>,
) -> Result<Vec<RequestReplay>> {
    let mut requests = Vec::new();

    let paths = spec
        .get("paths")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();

    for (path, path_item) in paths {
        let path_item_params = path_item
            .get("parameters")
            .and_then(|v| v.as_array())
            .cloned();
        let path_item_tags = path_item.get("tags").and_then(|v| v.as_array()).cloned();

        if let Some(methods) = path_item.as_object() {
            for (method, op) in methods {
                if !is_http_method(method) {
                    continue;
                }
                let op_obj = op.as_object().cloned().unwrap_or_default();
                if !include_private && is_private_operation(&op_obj, path_item_tags.as_ref()) {
                    continue;
                }

                let operation_id = op_obj
                    .get("operationId")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| format!("{} {}", method, path));

                let id = slugify(&operation_id);
                let name = op_obj
                    .get("summary")
                    .and_then(|v| v.as_str())
                    .unwrap_or(&operation_id)
                    .to_string();
                let description = op_obj
                    .get("description")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());

                let tags = op_obj
                    .get("tags")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_default();

                let params =
                    collect_parameters(path_item_params.as_ref(), op_obj.get("parameters"));
                let (path_params, query, headers) = split_parameters(params);

                let (body, body_example) = build_body_spec(
                    op_obj.get("requestBody"),
                    include_full,
                    output_dir,
                    overwrite,
                    &id,
                    schema_map,
                )?;

                let expected = build_expected_response(
                    op_obj.get("responses"),
                    include_full,
                    output_dir,
                    overwrite,
                    &id,
                    schema_map,
                )?;

                let auth = op_obj
                    .get("security")
                    .and_then(|v| v.as_array())
                    .map(|arr| !arr.is_empty())
                    .unwrap_or(true)
                    .then(|| RequestAuth {
                        scheme_id: "bearerAuth".to_string(),
                        required: true,
                    });

                let path_param_map: BTreeMap<String, serde_json::Value> = path_params
                    .iter()
                    .filter_map(|p| p.example.clone().map(|v| (p.name.clone(), v)))
                    .collect();

                let happy_query: BTreeMap<String, serde_json::Value> = query
                    .iter()
                    .filter_map(|p| p.example.clone().map(|v| (p.name.clone(), v)))
                    .collect();

                let happy_variant = RequestVariant {
                    id: "happy-path".to_string(),
                    name: "Happy path".to_string(),
                    description: Some("Expected successful request".to_string()),
                    request: VariantRequest {
                        path_params: path_param_map.clone(),
                        query: happy_query,
                        headers: headers
                            .iter()
                            .filter_map(|p| p.example.clone().map(|v| (p.name.clone(), v)))
                            .collect(),
                        body: body_example,
                    },
                };

                let edge_variant = RequestVariant {
                    id: "edge-case".to_string(),
                    name: "Edge case".to_string(),
                    description: Some("Missing optional fields or empty payload".to_string()),
                    request: VariantRequest {
                        path_params: path_param_map,
                        query: BTreeMap::new(),
                        headers: BTreeMap::new(),
                        body: None,
                    },
                };

                let request = RequestReplay {
                    id: id.clone(),
                    name,
                    method: method.to_uppercase(),
                    path: path.to_string(),
                    tags,
                    description,
                    auth,
                    path_params,
                    query,
                    headers,
                    body,
                    variants: vec![happy_variant, edge_variant],
                    expected,
                };

                let request_path = output_dir.join("requests").join(format!("{id}.json"));
                write_json(&request_path, &request, overwrite)?;
                requests.push(request);
            }
        }
    }

    Ok(requests)
}

fn collect_parameters(path_params: Option<&Vec<Value>>, op_params: Option<&Value>) -> Vec<Value> {
    let mut params = Vec::new();
    if let Some(path_params) = path_params {
        params.extend(path_params.clone());
    }
    if let Some(op_params) = op_params.and_then(|v| v.as_array()) {
        params.extend(op_params.clone());
    }
    params
}

fn split_parameters(params: Vec<Value>) -> (Vec<PathParam>, Vec<QueryParam>, Vec<HeaderParam>) {
    let mut path_params = Vec::new();
    let mut query = Vec::new();
    let mut headers = Vec::new();

    for param in params {
        let in_value = param.get("in").and_then(|v| v.as_str()).unwrap_or("");
        let name = param
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("param")
            .to_string();
        let required = param
            .get("required")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let description = param
            .get("description")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let example = param
            .get("example")
            .cloned()
            .or_else(|| param.get("schema").and_then(example_from_schema));

        if in_value == "header" {
            headers.push(HeaderParam {
                name,
                required,
                example,
                description,
            });
        } else if in_value == "path" {
            path_params.push(PathParam {
                name,
                required,
                example,
                description,
            });
        } else if in_value == "query" {
            query.push(QueryParam {
                name,
                required,
                example,
                description,
            });
        }
    }

    (path_params, query, headers)
}

fn build_body_spec(
    request_body: Option<&Value>,
    include_full: bool,
    output_dir: &Path,
    overwrite: bool,
    request_id: &str,
    schema_map: &mut HashMap<String, String>,
) -> Result<(Option<BodySpec>, Option<Value>)> {
    let Some(rb) = request_body else {
        return Ok((None, None));
    };

    let content = rb.get("content").and_then(|v| v.as_object());
    let Some(content) = content else {
        return Ok((None, None));
    };

    let (content_type, media) = content
        .iter()
        .next()
        .map(|(k, v)| (k.to_string(), v))
        .unwrap_or_else(|| ("application/json".to_string(), &Value::Null));

    let example = media.get("example").cloned().or_else(|| {
        media
            .get("examples")
            .and_then(|v| v.as_object())
            .and_then(|examples| examples.values().next())
            .and_then(|ex| ex.get("value").cloned())
    });

    let schema = media.get("schema");
    let schema_ref = match schema {
        Some(s) => resolve_schema_ref(
            s,
            include_full,
            output_dir,
            overwrite,
            format!("{request_id}-request"),
            schema_map,
        )?,
        None => None,
    };

    Ok((
        Some(BodySpec {
            content_type,
            example: example
                .clone()
                .or_else(|| schema.and_then(example_from_schema)),
            schema_ref,
        }),
        example,
    ))
}

fn build_expected_response(
    responses: Option<&Value>,
    include_full: bool,
    output_dir: &Path,
    overwrite: bool,
    request_id: &str,
    schema_map: &mut HashMap<String, String>,
) -> Result<ExpectedResponse> {
    let mut status = 200;
    let mut schema_ref = None;
    let mut content_type = None;

    if let Some(responses) = responses.and_then(|v| v.as_object()) {
        let status_key = responses
            .keys()
            .find(|k| k.starts_with('2'))
            .cloned()
            .unwrap_or_else(|| "200".to_string());

        if let Ok(code) = status_key.parse::<u16>() {
            status = code;
        }

        if let Some(resp) = responses.get(&status_key)
            && let Some(content) = resp.get("content").and_then(|v| v.as_object())
            && let Some((ct, media)) = content.iter().next()
        {
            content_type = Some(ct.to_string());
            if let Some(schema) = media.get("schema") {
                schema_ref = resolve_schema_ref(
                    schema,
                    include_full,
                    output_dir,
                    overwrite,
                    format!("{request_id}-response"),
                    schema_map,
                )?;
                if schema_ref.is_none() && !include_full {
                    schema_ref = resolve_schema_ref(
                        schema,
                        true,
                        output_dir,
                        overwrite,
                        format!("{request_id}-response"),
                        schema_map,
                    )?;
                }
            }
        }
    }

    Ok(ExpectedResponse {
        status,
        content_type,
        schema_ref,
    })
}

fn resolve_schema_ref(
    schema: &Value,
    include_full: bool,
    output_dir: &Path,
    overwrite: bool,
    inline_id: String,
    schema_map: &mut HashMap<String, String>,
) -> Result<Option<String>> {
    if let Some(ref_str) = schema.get("$ref").and_then(|v| v.as_str()) {
        let name = ref_str
            .split('/')
            .next_back()
            .unwrap_or(ref_str)
            .to_string();
        if let Some(path) = schema_map.get(&name) {
            return Ok(Some(path.clone()));
        }
        let file_name = format!("{name}.json");
        let path = output_dir.join("schemas").join(&file_name);
        if write_json(&path, schema, overwrite)? {
            let rel = format!("schemas/{file_name}");
            schema_map.insert(name.clone(), rel.clone());
            return Ok(Some(rel));
        }
        return Ok(Some(format!("schemas/{file_name}")));
    }

    if include_full {
        let file_name = format!("inline-{inline_id}.json");
        let path = output_dir.join("schemas").join(&file_name);
        write_json(&path, schema, overwrite)?;
        return Ok(Some(format!("schemas/{file_name}")));
    }

    Ok(None)
}

fn example_from_schema(schema: &Value) -> Option<Value> {
    if let Some(example) = schema.get("example") {
        return Some(example.clone());
    }
    if let Some(default) = schema.get("default") {
        return Some(default.clone());
    }
    if let Some(enums) = schema.get("enum").and_then(|v| v.as_array()) {
        return enums.first().cloned();
    }

    match schema.get("type").and_then(|v| v.as_str()) {
        Some("string") => Some(Value::String("example".to_string())),
        Some("integer") => Some(Value::Number(0.into())),
        Some("number") => Some(Value::Number(0.into())),
        Some("boolean") => Some(Value::Bool(true)),
        Some("array") => Some(Value::Array(Vec::new())),
        Some("object") => Some(Value::Object(serde_json::Map::new())),
        _ => None,
    }
}

fn is_http_method(method: &str) -> bool {
    matches!(
        method.to_lowercase().as_str(),
        "get" | "post" | "put" | "patch" | "delete" | "options" | "head"
    )
}

fn is_private_operation(op: &Map<String, Value>, path_tags: Option<&Vec<Value>>) -> bool {
    let tag_values = op
        .get("tags")
        .and_then(|v| v.as_array())
        .or(path_tags)
        .cloned()
        .unwrap_or_default();

    let tag_set: Vec<String> = tag_values
        .iter()
        .filter_map(|v| v.as_str().map(|s| s.to_lowercase()))
        .collect();

    let has_private_tag = tag_set
        .iter()
        .any(|t| t.contains("internal") || t.contains("private"));
    let x_internal = op
        .get("x-internal")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    has_private_tag || x_internal
}

fn slugify(input: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in input.chars() {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_alphanumeric() {
            out.push(lower);
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

fn ensure_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path)
        .with_context(|| format!("failed to create directory {}", path.display()))?;
    Ok(())
}

fn write_json(path: &Path, value: &impl serde::Serialize, overwrite: bool) -> Result<bool> {
    if path.exists() && !overwrite {
        return Ok(false);
    }
    let payload = serde_json::to_string_pretty(value)?;
    fs::write(path, payload).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_slugify() {
        assert_eq!(slugify("getUser"), "getuser");
        assert_eq!(slugify("GET /users/{id}"), "get-users-id");
        assert_eq!(slugify("hello world"), "hello-world");
        assert_eq!(slugify("--foo--"), "foo");
        assert_eq!(slugify("CamelCase"), "camelcase");
    }

    #[test]
    fn test_example_from_schema_types() {
        assert_eq!(
            example_from_schema(&json!({"type": "string"})),
            Some(json!("example"))
        );
        assert_eq!(
            example_from_schema(&json!({"type": "integer"})),
            Some(json!(0))
        );
        assert_eq!(
            example_from_schema(&json!({"type": "boolean"})),
            Some(json!(true))
        );
        assert_eq!(
            example_from_schema(&json!({"type": "array"})),
            Some(json!([]))
        );
        assert_eq!(
            example_from_schema(&json!({"type": "object"})),
            Some(json!({}))
        );
        assert_eq!(example_from_schema(&json!({})), None);
    }

    #[test]
    fn test_example_from_schema_explicit() {
        assert_eq!(
            example_from_schema(&json!({"example": "foo"})),
            Some(json!("foo"))
        );
        assert_eq!(
            example_from_schema(&json!({"default": 42})),
            Some(json!(42))
        );
        assert_eq!(
            example_from_schema(&json!({"enum": ["a", "b"]})),
            Some(json!("a"))
        );
    }

    #[test]
    fn test_split_parameters() {
        let params = vec![
            json!({"name": "id", "in": "path", "required": true}),
            json!({"name": "limit", "in": "query", "required": false, "schema": {"type": "integer"}}),
            json!({"name": "X-Request-Id", "in": "header"}),
        ];
        let (path, query, headers) = split_parameters(params);
        assert_eq!(path.len(), 1);
        assert_eq!(path[0].name, "id");
        assert!(path[0].required);
        assert_eq!(query.len(), 1);
        assert_eq!(query[0].name, "limit");
        assert_eq!(headers.len(), 1);
        assert_eq!(headers[0].name, "X-Request-Id");
    }

    #[test]
    fn test_is_http_method() {
        assert!(is_http_method("get"));
        assert!(is_http_method("POST"));
        assert!(is_http_method("Delete"));
        assert!(!is_http_method("parameters"));
        assert!(!is_http_method("summary"));
    }

    #[test]
    fn test_is_private_operation() {
        let op: Map<String, Value> = serde_json::from_value(json!({"tags": ["internal"]})).unwrap();
        assert!(is_private_operation(&op, None));

        let op: Map<String, Value> = serde_json::from_value(json!({"x-internal": true})).unwrap();
        assert!(is_private_operation(&op, None));

        let op: Map<String, Value> = serde_json::from_value(json!({"tags": ["users"]})).unwrap();
        assert!(!is_private_operation(&op, None));
    }

    #[test]
    fn test_extract_auth_spec_with_schemes() {
        let spec = json!({
            "components": {
                "securitySchemes": {
                    "apiKey": {
                        "type": "apiKey",
                        "name": "X-API-Key",
                        "in": "header"
                    }
                }
            }
        });
        let auth = extract_auth_spec(&spec);
        assert_eq!(auth.schemes.len(), 1);
        assert_eq!(auth.schemes[0].id, "apiKey");
        assert_eq!(auth.default, Some("apiKey".to_string()));
    }

    #[test]
    fn test_extract_auth_spec_fallback() {
        let spec = json!({});
        let auth = extract_auth_spec(&spec);
        assert_eq!(auth.schemes.len(), 1);
        assert_eq!(auth.schemes[0].id, "bearerAuth");
        assert_eq!(auth.default, Some("bearerAuth".to_string()));
    }
}
