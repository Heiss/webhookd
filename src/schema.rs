//! JSON Schema validation with `assign_value` variable extraction.
//!
//! webhookd extends standard JSON Schema with a custom keyword:
//! **`assign_value`**.  When a property definition includes
//! `"assign_value": "some-name"`, the value of that property in incoming
//! payloads is captured into a variable map.  Those variables can later be
//! interpolated into an output template (see [`crate::template`]).
//!
//! # How `assign_value` works
//!
//! Given an input schema:
//!
//! ```json
//! {
//!   "type": "object",
//!   "properties": {
//!     "productId": {
//!       "type": "integer",
//!       "assign_value": "my-product-id"
//!     },
//!     "name": {
//!       "type": "string",
//!       "assign_value": "product-name"
//!     }
//!   }
//! }
//! ```
//!
//! And an incoming payload:
//!
//! ```json
//! { "productId": 42, "name": "Widget" }
//! ```
//!
//! The extractor produces the variable map:
//!
//! ```text
//! { "my-product-id": 42, "product-name": "Widget" }
//! ```
//!
//! These variables are then available in the output template as
//! `{{ my-product-id }}` and `{{ product-name }}`.
//!
//! # Nested objects
//!
//! `assign_value` is supported at any depth.  For nested objects, only the
//! leaf properties that define `assign_value` are extracted.

use serde_json::Value;
use std::collections::HashMap;
use thiserror::Error;

/// Errors produced during schema operations.
#[derive(Debug, Error)]
pub enum SchemaError {
    #[error("input schema is not valid JSON: {0}")]
    InvalidSchemaJson(#[from] serde_json::Error),

    #[error("JSON Schema compilation failed: {0}")]
    CompilationFailed(String),

    #[error("payload validation failed: {0}")]
    ValidationFailed(String),

    #[error("payload is not valid JSON: {0}")]
    InvalidPayloadJson(serde_json::Error),
}

/// A compiled input schema that can validate payloads and extract variables.
#[derive(Debug, Clone)]
pub struct InputSchema {
    /// The raw JSON Schema value (needed for validation).
    schema_value: Value,
    /// Map from JSON-pointer-like dotted paths to the `assign_value` names.
    /// E.g.  `"productId"` → `"my-product-id"`.
    assignments: HashMap<String, String>,
}

impl InputSchema {
    /// Parse and compile a JSON Schema string.
    ///
    /// Extracts all `assign_value` annotations from property definitions so
    /// they can be used later in [`Self::validate_and_extract`].
    pub fn compile(schema_json: &str) -> Result<Self, SchemaError> {
        let schema_value: Value = serde_json::from_str(schema_json)?;

        // Walk the schema and collect assign_value mappings.
        let mut assignments = HashMap::new();
        collect_assignments(&schema_value, "", &mut assignments);

        // Verify that jsonschema can actually compile it (ignoring the custom keyword).
        if let Err(e) = jsonschema::validator_for(&schema_value) {
            return Err(SchemaError::CompilationFailed(e.to_string()));
        }

        Ok(Self {
            schema_value,
            assignments,
        })
    }

    /// Validate a JSON payload against this schema and extract variables.
    ///
    /// Returns a map of variable names (from `assign_value`) to their values
    /// in the payload.
    pub fn validate_and_extract(
        &self,
        payload: &str,
    ) -> Result<HashMap<String, Value>, SchemaError> {
        let instance: Value =
            serde_json::from_str(payload).map_err(SchemaError::InvalidPayloadJson)?;

        // Validate
        let validator = jsonschema::validator_for(&self.schema_value)
            .map_err(|e| SchemaError::CompilationFailed(e.to_string()))?;

        let errors: Vec<String> = validator
            .iter_errors(&instance)
            .map(|e| e.to_string())
            .collect();

        if !errors.is_empty() {
            return Err(SchemaError::ValidationFailed(errors.join("; ")));
        }

        // Extract assigned values
        let mut vars = HashMap::new();
        for (json_path, var_name) in &self.assignments {
            if let Some(val) = resolve_path(&instance, json_path) {
                vars.insert(var_name.clone(), val.clone());
            }
        }

        Ok(vars)
    }

    /// Return the raw assignment map (path → variable name).
    pub fn assignments(&self) -> &HashMap<String, String> {
        &self.assignments
    }
}

/// Recursively walk a JSON Schema value and collect `assign_value` annotations
/// from property definitions.
///
/// `current_path` tracks the dotted path from the root `properties` object.
fn collect_assignments(schema: &Value, current_path: &str, out: &mut HashMap<String, String>) {
    if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
        for (prop_name, prop_schema) in properties {
            let path = if current_path.is_empty() {
                prop_name.clone()
            } else {
                format!("{current_path}.{prop_name}")
            };

            // Check for assign_value at this level
            if let Some(assign) = prop_schema.get("assign_value").and_then(Value::as_str) {
                out.insert(path.clone(), assign.to_string());
            }

            // Recurse into nested objects
            collect_assignments(prop_schema, &path, out);
        }
    }

    // Handle items in arrays
    if let Some(items) = schema.get("items") {
        collect_assignments(items, current_path, out);
    }
}

/// Resolve a dotted path (e.g. `"foo.bar.baz"`) against a JSON value.
fn resolve_path<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = value;
    for segment in path.split('.') {
        current = current.get(segment)?;
    }
    Some(current)
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn simple_schema() -> &'static str {
        r#"{
            "type": "object",
            "properties": {
                "productId": {
                    "type": "integer",
                    "assign_value": "my-product-id"
                },
                "name": {
                    "type": "string",
                    "assign_value": "product-name"
                }
            },
            "required": ["productId"]
        }"#
    }

    // ── compile ─────────────────────────────────────────────────────────

    #[test]
    fn compile_valid_schema() {
        let schema = InputSchema::compile(simple_schema()).unwrap();
        assert_eq!(schema.assignments().len(), 2);
        assert_eq!(
            schema.assignments().get("productId"),
            Some(&"my-product-id".to_string())
        );
        assert_eq!(
            schema.assignments().get("name"),
            Some(&"product-name".to_string())
        );
    }

    #[test]
    fn compile_invalid_json_is_error() {
        let err = InputSchema::compile("not json").unwrap_err();
        assert!(matches!(err, SchemaError::InvalidSchemaJson(_)));
    }

    #[test]
    fn compile_schema_without_assign_value() {
        let schema_str = r#"{"type": "object", "properties": {"id": {"type": "integer"}}}"#;
        let schema = InputSchema::compile(schema_str).unwrap();
        assert!(schema.assignments().is_empty());
    }

    // ── validate_and_extract ────────────────────────────────────────────

    #[test]
    fn extract_values_from_valid_payload() {
        let schema = InputSchema::compile(simple_schema()).unwrap();
        let payload = r#"{"productId": 42, "name": "Widget"}"#;
        let vars = schema.validate_and_extract(payload).unwrap();

        assert_eq!(vars.get("my-product-id"), Some(&json!(42)));
        assert_eq!(vars.get("product-name"), Some(&json!("Widget")));
    }

    #[test]
    fn extract_only_present_optional_fields() {
        let schema = InputSchema::compile(simple_schema()).unwrap();
        // name is not required, so omit it
        let payload = r#"{"productId": 7}"#;
        let vars = schema.validate_and_extract(payload).unwrap();

        assert_eq!(vars.get("my-product-id"), Some(&json!(7)));
        assert!(
            !vars.contains_key("product-name"),
            "absent optional fields should not appear in vars"
        );
    }

    #[test]
    fn validation_fails_for_missing_required_field() {
        let schema = InputSchema::compile(simple_schema()).unwrap();
        let payload = r#"{"name": "Widget"}"#;
        let err = schema.validate_and_extract(payload).unwrap_err();
        assert!(matches!(err, SchemaError::ValidationFailed(_)));
    }

    #[test]
    fn validation_fails_for_wrong_type() {
        let schema = InputSchema::compile(simple_schema()).unwrap();
        let payload = r#"{"productId": "not-an-integer"}"#;
        let err = schema.validate_and_extract(payload).unwrap_err();
        assert!(matches!(err, SchemaError::ValidationFailed(_)));
    }

    #[test]
    fn invalid_payload_json_is_error() {
        let schema = InputSchema::compile(simple_schema()).unwrap();
        let err = schema.validate_and_extract("not json").unwrap_err();
        assert!(matches!(err, SchemaError::InvalidPayloadJson(_)));
    }

    // ── nested objects ──────────────────────────────────────────────────

    #[test]
    fn extract_from_nested_object() {
        let schema_str = r#"{
            "type": "object",
            "properties": {
                "order": {
                    "type": "object",
                    "properties": {
                        "id": {
                            "type": "integer",
                            "assign_value": "order-id"
                        },
                        "customer": {
                            "type": "object",
                            "properties": {
                                "email": {
                                    "type": "string",
                                    "assign_value": "customer-email"
                                }
                            }
                        }
                    }
                }
            }
        }"#;

        let schema = InputSchema::compile(schema_str).unwrap();
        assert_eq!(schema.assignments().len(), 2);
        assert_eq!(
            schema.assignments().get("order.id"),
            Some(&"order-id".to_string())
        );
        assert_eq!(
            schema.assignments().get("order.customer.email"),
            Some(&"customer-email".to_string())
        );

        let payload = r#"{"order": {"id": 99, "customer": {"email": "a@b.com"}}}"#;
        let vars = schema.validate_and_extract(payload).unwrap();
        assert_eq!(vars.get("order-id"), Some(&json!(99)));
        assert_eq!(vars.get("customer-email"), Some(&json!("a@b.com")));
    }

    // ── boolean / null / array values ───────────────────────────────────

    #[test]
    fn extract_boolean_value() {
        let schema_str = r#"{
            "type": "object",
            "properties": {
                "active": {
                    "type": "boolean",
                    "assign_value": "is-active"
                }
            }
        }"#;
        let schema = InputSchema::compile(schema_str).unwrap();
        let vars = schema.validate_and_extract(r#"{"active": true}"#).unwrap();
        assert_eq!(vars.get("is-active"), Some(&json!(true)));
    }

    #[test]
    fn extract_null_value() {
        let schema_str = r#"{
            "type": "object",
            "properties": {
                "data": {
                    "type": "null",
                    "assign_value": "null-val"
                }
            }
        }"#;
        let schema = InputSchema::compile(schema_str).unwrap();
        let vars = schema.validate_and_extract(r#"{"data": null}"#).unwrap();
        assert_eq!(vars.get("null-val"), Some(&json!(null)));
    }

    #[test]
    fn extract_float_value() {
        let schema_str = r#"{
            "type": "object",
            "properties": {
                "price": {
                    "type": "number",
                    "assign_value": "item-price"
                }
            }
        }"#;
        let schema = InputSchema::compile(schema_str).unwrap();
        let vars = schema.validate_and_extract(r#"{"price": 19.99}"#).unwrap();
        assert_eq!(vars.get("item-price"), Some(&json!(19.99)));
    }

    // ── resolve_path ────────────────────────────────────────────────────

    #[test]
    fn resolve_path_simple() {
        let v = json!({"a": 1});
        assert_eq!(resolve_path(&v, "a"), Some(&json!(1)));
    }

    #[test]
    fn resolve_path_nested() {
        let v = json!({"a": {"b": {"c": 42}}});
        assert_eq!(resolve_path(&v, "a.b.c"), Some(&json!(42)));
    }

    #[test]
    fn resolve_path_missing_returns_none() {
        let v = json!({"a": 1});
        assert_eq!(resolve_path(&v, "b"), None);
    }

    // ── collect_assignments ─────────────────────────────────────────────

    #[test]
    fn collect_assignments_empty_schema() {
        let schema = json!({});
        let mut out = HashMap::new();
        collect_assignments(&schema, "", &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn collect_assignments_no_assign_value() {
        let schema = json!({
            "type": "object",
            "properties": {
                "id": {"type": "integer"}
            }
        });
        let mut out = HashMap::new();
        collect_assignments(&schema, "", &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn collect_assignments_flat() {
        let schema = json!({
            "type": "object",
            "properties": {
                "x": {"type": "integer", "assign_value": "var-x"},
                "y": {"type": "string", "assign_value": "var-y"}
            }
        });
        let mut out = HashMap::new();
        collect_assignments(&schema, "", &mut out);
        assert_eq!(out.len(), 2);
        assert_eq!(out.get("x"), Some(&"var-x".to_string()));
        assert_eq!(out.get("y"), Some(&"var-y".to_string()));
    }

    #[test]
    fn collect_assignments_nested() {
        let schema = json!({
            "type": "object",
            "properties": {
                "outer": {
                    "type": "object",
                    "properties": {
                        "inner": {
                            "type": "string",
                            "assign_value": "deep-var"
                        }
                    }
                }
            }
        });
        let mut out = HashMap::new();
        collect_assignments(&schema, "", &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out.get("outer.inner"), Some(&"deep-var".to_string()));
    }

    // ── end-to-end: schema from config example ──────────────────────────

    #[test]
    fn config_example_schema_works() {
        let schema_str = r#"{
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "$id": "https://example.com/product.schema.json",
            "title": "Product",
            "description": "A product from Acme's catalog",
            "type": "object",
            "properties": {
                "productId": {
                    "description": "The unique identifier for a product",
                    "type": "integer",
                    "assign_value": "my-key"
                }
            }
        }"#;

        let schema = InputSchema::compile(schema_str).unwrap();
        assert_eq!(schema.assignments().len(), 1);
        assert_eq!(
            schema.assignments().get("productId"),
            Some(&"my-key".to_string())
        );

        let payload = r#"{"productId": 123}"#;
        let vars = schema.validate_and_extract(payload).unwrap();
        assert_eq!(vars.get("my-key"), Some(&json!(123)));
    }
}
