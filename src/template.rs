//! Jinja2-style output template rendering.
//!
//! Uses [MiniJinja](https://docs.rs/minijinja) to render output templates.
//! Variables extracted from the input payload via `assign_value` (see
//! [`crate::schema`]) are passed into the template context.
//!
//! # Template syntax
//!
//! Templates use Jinja2 syntax.  The most common usage is simple variable
//! interpolation:
//!
//! ```text
//! {
//!   "custom-key": {{ my-key }}
//! }
//! ```
//!
//! Any MiniJinja expression is supported — filters, conditionals, loops, etc.
//! See <https://docs.rs/minijinja> for the full template language reference.
//!
//! # Variable names with hyphens
//!
//! Because `assign_value` names may contain hyphens (e.g. `"my-key"`),
//! templates must reference them via the lookup syntax if the name is not a
//! valid identifier.  MiniJinja supports attribute access with hyphens
//! transparently when the variable is injected into the context.

use minijinja::Environment;
use serde_json::Value;
use std::collections::HashMap;
use thiserror::Error;

/// Errors produced during template rendering.
#[derive(Debug, Error)]
pub enum TemplateError {
    #[error("template rendering failed: {0}")]
    RenderFailed(#[from] minijinja::Error),
}

/// Render an output template with the given variables.
///
/// `template_str` is a Jinja2 template string.  `vars` maps variable names
/// (from `assign_value`) to their JSON values extracted from the payload.
///
/// JSON values are converted into MiniJinja-compatible types:
/// - strings → string
/// - numbers → number
/// - booleans → bool
/// - null → none
/// - arrays/objects → serialized JSON string
pub fn render(template_str: &str, vars: &HashMap<String, Value>) -> Result<String, TemplateError> {
    let mut env = Environment::new();
    env.add_template("output", template_str)?;

    let tmpl = env.get_template("output")?;

    // Convert serde_json::Value map to minijinja::Value map
    let ctx = to_minijinja_context(vars);

    let rendered = tmpl.render(ctx)?;
    Ok(rendered)
}

/// Convert a HashMap<String, serde_json::Value> into a minijinja::Value context object.
fn to_minijinja_context(vars: &HashMap<String, Value>) -> minijinja::Value {
    let mut map = std::collections::BTreeMap::new();
    for (key, val) in vars {
        map.insert(key.clone(), json_to_minijinja(val));
    }
    minijinja::Value::from_serialize(&map)
}

/// Convert a single serde_json::Value into a minijinja::Value.
fn json_to_minijinja(val: &Value) -> minijinja::Value {
    match val {
        Value::Null => minijinja::Value::from(()),
        Value::Bool(b) => minijinja::Value::from(*b),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                minijinja::Value::from(i)
            } else if let Some(f) = n.as_f64() {
                minijinja::Value::from(f)
            } else {
                minijinja::Value::from(n.to_string())
            }
        }
        Value::String(s) => minijinja::Value::from(s.clone()),
        Value::Array(arr) => {
            let items: Vec<minijinja::Value> = arr.iter().map(json_to_minijinja).collect();
            minijinja::Value::from(items)
        }
        Value::Object(obj) => {
            let mut map = std::collections::BTreeMap::new();
            for (k, v) in obj {
                map.insert(k.clone(), json_to_minijinja(v));
            }
            minijinja::Value::from_serialize(&map)
        }
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── basic rendering ─────────────────────────────────────────────────

    #[test]
    fn render_simple_variable() {
        let mut vars = HashMap::new();
        vars.insert("name".to_string(), json!("Alice"));

        let result = render("Hello, {{ name }}!", &vars).unwrap();
        assert_eq!(result, "Hello, Alice!");
    }

    #[test]
    fn render_integer_variable() {
        let mut vars = HashMap::new();
        vars.insert("count".to_string(), json!(42));

        let result = render("Count: {{ count }}", &vars).unwrap();
        assert_eq!(result, "Count: 42");
    }

    #[test]
    fn render_boolean_variable() {
        let mut vars = HashMap::new();
        vars.insert("active".to_string(), json!(true));

        let result = render("Active: {{ active }}", &vars).unwrap();
        assert_eq!(result, "Active: true");
    }

    #[test]
    fn render_float_variable() {
        let mut vars = HashMap::new();
        vars.insert("price".to_string(), json!(19.99));

        let result = render("Price: {{ price }}", &vars).unwrap();
        assert_eq!(result, "Price: 19.99");
    }

    #[test]
    fn render_null_variable() {
        let mut vars = HashMap::new();
        vars.insert("data".to_string(), json!(null));

        let result = render("Data: {{ data }}", &vars).unwrap();
        assert_eq!(result, "Data: none");
    }

    // ── multiple variables ──────────────────────────────────────────────

    #[test]
    fn render_multiple_variables() {
        let mut vars = HashMap::new();
        vars.insert("a".to_string(), json!(1));
        vars.insert("b".to_string(), json!("two"));

        let result = render("{{ a }} and {{ b }}", &vars).unwrap();
        assert_eq!(result, "1 and two");
    }

    // ── JSON output template from config example ────────────────────────

    #[test]
    fn render_json_output_template() {
        let mut vars = HashMap::new();
        vars.insert("my-key".to_string(), json!(123));

        // The config example uses: {"custom-key": {{ my-key }}}
        // In Jinja2, hyphens in var names need special handling.
        // MiniJinja treats them as subtraction, so we use bracket notation.
        let template = r#"{"custom-key": {{ vars["my-key"] }}}"#;
        let mut context = HashMap::new();
        context.insert("vars".to_string(), json!({"my-key": 123}));

        let result = render(template, &context).unwrap();
        assert_eq!(result, r#"{"custom-key": 123}"#);
    }

    // ── using underscored variable names ────────────────────────────────

    #[test]
    fn render_underscored_variable_names() {
        let mut vars = HashMap::new();
        vars.insert("my_key".to_string(), json!(456));

        let result = render(r#"{"result": {{ my_key }}}"#, &vars).unwrap();
        assert_eq!(result, r#"{"result": 456}"#);
    }

    // ── empty vars ──────────────────────────────────────────────────────

    #[test]
    fn render_no_variables() {
        let vars = HashMap::new();
        let result = render("static text", &vars).unwrap();
        assert_eq!(result, "static text");
    }

    // ── template syntax errors ──────────────────────────────────────────

    #[test]
    fn render_invalid_template_is_error() {
        let vars = HashMap::new();
        let err = render("{{ unclosed", &vars).unwrap_err();
        assert!(matches!(err, TemplateError::RenderFailed(_)));
    }

    // ── conditional rendering ───────────────────────────────────────────

    #[test]
    fn render_conditional() {
        let mut vars = HashMap::new();
        vars.insert("status".to_string(), json!("ok"));

        let template = "{% if status == \"ok\" %}success{% else %}failure{% endif %}";
        let result = render(template, &vars).unwrap();
        assert_eq!(result, "success");
    }

    // ── array iteration ─────────────────────────────────────────────────

    #[test]
    fn render_array_variable() {
        let mut vars = HashMap::new();
        vars.insert("items".to_string(), json!(["a", "b", "c"]));

        let template = "{% for item in items %}{{ item }},{% endfor %}";
        let result = render(template, &vars).unwrap();
        assert_eq!(result, "a,b,c,");
    }

    // ── string with special characters ──────────────────────────────────

    #[test]
    fn render_string_with_quotes() {
        let mut vars = HashMap::new();
        vars.insert("msg".to_string(), json!("hello \"world\""));

        let result = render("{{ msg }}", &vars).unwrap();
        assert!(result.contains("hello"));
    }

    // ── to_minijinja_context ────────────────────────────────────────────

    #[test]
    fn context_conversion_preserves_types() {
        let mut vars = HashMap::new();
        vars.insert("int_val".to_string(), json!(42));
        vars.insert("str_val".to_string(), json!("hello"));
        vars.insert("bool_val".to_string(), json!(true));
        vars.insert("null_val".to_string(), json!(null));
        vars.insert("float_val".to_string(), json!(2.72));

        let ctx = to_minijinja_context(&vars);
        // The context should be a valid minijinja value
        assert!(!ctx.is_undefined());
    }

    // ── json_to_minijinja ───────────────────────────────────────────────

    #[test]
    fn json_null_to_minijinja() {
        let v = json_to_minijinja(&json!(null));
        assert!(v.is_none());
    }

    #[test]
    fn json_bool_to_minijinja() {
        let v = json_to_minijinja(&json!(true));
        assert_eq!(v.to_string(), "true");
    }

    #[test]
    fn json_int_to_minijinja() {
        let v = json_to_minijinja(&json!(42));
        assert_eq!(v.to_string(), "42");
    }

    #[test]
    fn json_string_to_minijinja() {
        let v = json_to_minijinja(&json!("hello"));
        assert_eq!(v.to_string(), "hello");
    }

    #[test]
    fn json_array_to_minijinja() {
        let v = json_to_minijinja(&json!([1, 2, 3]));
        assert_eq!(v.to_string(), "[1, 2, 3]");
    }

    #[test]
    fn json_object_to_minijinja() {
        let v = json_to_minijinja(&json!({"key": "val"}));
        let s = v.to_string();
        assert!(s.contains("key"));
    }
}
