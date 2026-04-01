//! XSD-based XML schema validation with `assign_value` variable extraction.
//!
//! webhookd supports XML payloads validated against an XSD-like schema.  The
//! schema is extended with a custom namespace attribute
//! **`whd:assign_value`** (namespace `https://webhookd.dev/schema`) that
//! captures element values into a variable map for use in output templates.
//!
//! # How `whd:assign_value` works
//!
//! Given an input XSD schema:
//!
//! ```xml
//! <xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
//!            xmlns:whd="https://webhookd.dev/schema">
//!   <xs:element name="product">
//!     <xs:complexType>
//!       <xs:sequence>
//!         <xs:element name="productId" type="xs:integer"
//!                     whd:assign_value="my-product-id"/>
//!         <xs:element name="name" type="xs:string"
//!                     whd:assign_value="product-name"
//!                     minOccurs="0"/>
//!       </xs:sequence>
//!     </xs:complexType>
//!   </xs:element>
//! </xs:schema>
//! ```
//!
//! And an incoming XML payload:
//!
//! ```xml
//! <product>
//!   <productId>42</productId>
//!   <name>Widget</name>
//! </product>
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
//! # Supported XSD features
//!
//! This is a lightweight XSD validator focused on webhook payloads.  Supported:
//!
//! - Element type checking: `xs:string`, `xs:integer`, `xs:decimal`,
//!   `xs:boolean`, `xs:float`, `xs:double`, `xs:date`, `xs:dateTime`
//! - Required / optional elements via `minOccurs` (default: 1 = required)
//! - Nested `xs:complexType` with `xs:sequence`
//! - Custom `whd:assign_value` attribute on `xs:element`
//!
//! # Nested elements
//!
//! `whd:assign_value` is supported at any depth.  For nested complex types,
//! only the leaf elements that define `assign_value` are extracted.

use roxmltree::{Document, Node};
use serde_json::Value;
use std::collections::HashMap;
use thiserror::Error;

/// The webhookd custom namespace URI for the `assign_value` attribute.
const WHD_NS: &str = "https://webhookd.dev/schema";

/// The XML Schema namespace URI.
const XS_NS: &str = "http://www.w3.org/2001/XMLSchema";

/// Errors produced during XML schema operations.
#[derive(Debug, Error)]
pub enum XmlSchemaError {
    #[error("XSD schema is not valid XML: {0}")]
    InvalidSchemaXml(String),

    #[error("XSD schema structure error: {0}")]
    SchemaStructure(String),

    #[error("XML payload is not valid: {0}")]
    InvalidPayloadXml(String),

    #[error("XML validation failed: {0}")]
    ValidationFailed(String),
}

// ─── Schema element definition ──────────────────────────────────────────────

/// Represents a single element definition from the XSD schema.
#[derive(Debug, Clone)]
struct ElementDef {
    /// Element name.
    name: String,
    /// XSD type (e.g. `"xs:string"`, `"xs:integer"`).  Empty for complex types.
    xsd_type: Option<String>,
    /// Whether this element is required (minOccurs >= 1).
    required: bool,
    /// Variable name to assign extracted value to.
    assign_value: Option<String>,
    /// Child element definitions (for complex types).
    children: Vec<ElementDef>,
}

// ─── Compiled XML schema ────────────────────────────────────────────────────

/// A compiled XSD input schema that can validate XML payloads and extract variables.
#[derive(Debug, Clone)]
pub struct XmlInputSchema {
    /// The root element definition parsed from the XSD.
    root: ElementDef,
    /// Map of dotted paths → `assign_value` names, collected at compile time.
    assignments: HashMap<String, String>,
}

impl XmlInputSchema {
    /// Parse and compile an XSD schema string.
    ///
    /// Extracts all `whd:assign_value` annotations from element definitions
    /// so they can be used later in [`Self::validate_and_extract`].
    pub fn compile(xsd_str: &str) -> Result<Self, XmlSchemaError> {
        let doc = Document::parse(xsd_str)
            .map_err(|e| XmlSchemaError::InvalidSchemaXml(e.to_string()))?;

        let root_node = doc.root_element();

        // Find the root xs:element
        let root_element_node =
            find_child_in_ns(&root_node, XS_NS, "element").ok_or_else(|| {
                XmlSchemaError::SchemaStructure("XSD must contain a root xs:element".into())
            })?;

        let root = parse_element_def(&root_element_node)?;

        // Collect assignments
        let mut assignments = HashMap::new();
        collect_xml_assignments(&root, "", &mut assignments);

        Ok(Self { root, assignments })
    }

    /// Validate an XML payload against this schema and extract variables.
    ///
    /// Returns a map of variable names (from `whd:assign_value`) to their
    /// values in the payload as `serde_json::Value`.
    pub fn validate_and_extract(
        &self,
        payload: &str,
    ) -> Result<HashMap<String, Value>, XmlSchemaError> {
        let doc = Document::parse(payload)
            .map_err(|e| XmlSchemaError::InvalidPayloadXml(e.to_string()))?;

        let root_node = doc.root_element();

        // Check root element name
        if root_node.tag_name().name() != self.root.name {
            return Err(XmlSchemaError::ValidationFailed(format!(
                "expected root element '{}', got '{}'",
                self.root.name,
                root_node.tag_name().name()
            )));
        }

        // Validate and extract
        let mut vars = HashMap::new();
        validate_element(&root_node, &self.root, &mut vars)?;

        Ok(vars)
    }

    /// Return the raw assignment map (path → variable name).
    pub fn assignments(&self) -> &HashMap<String, String> {
        &self.assignments
    }
}

// ─── Schema parsing ─────────────────────────────────────────────────────────

/// Parse an `xs:element` node into an [`ElementDef`].
fn parse_element_def(node: &Node) -> Result<ElementDef, XmlSchemaError> {
    let name = node
        .attribute("name")
        .ok_or_else(|| {
            XmlSchemaError::SchemaStructure("xs:element must have a 'name' attribute".into())
        })?
        .to_string();

    let xsd_type = node.attribute("type").map(|s| s.to_string());

    let required = node
        .attribute("minOccurs")
        .map(|v| v != "0")
        .unwrap_or(true);

    // Look for whd:assign_value attribute
    let assign_value = node
        .attribute((WHD_NS, "assign_value"))
        .or_else(|| {
            // Also check for prefixed assign_value for convenience
            node.attributes()
                .find(|a| a.name() == "assign_value" || a.name() == "whd:assign_value")
                .map(|a| a.value())
        })
        .map(|s| s.to_string());

    // Parse children if there's a complexType
    let mut children = Vec::new();
    if let Some(complex_type) = find_child_in_ns(node, XS_NS, "complexType") {
        if let Some(sequence) = find_child_in_ns(&complex_type, XS_NS, "sequence") {
            for child in sequence.children() {
                if child.is_element()
                    && child.tag_name().name() == "element"
                    && (child.tag_name().namespace() == Some(XS_NS)
                        || child.tag_name().namespace().is_none())
                {
                    children.push(parse_element_def(&child)?);
                }
            }
        }
    }

    Ok(ElementDef {
        name,
        xsd_type,
        required,
        assign_value,
        children,
    })
}

/// Find a child element with a specific namespace and local name.
fn find_child_in_ns<'a>(parent: &'a Node, ns: &str, local_name: &str) -> Option<Node<'a, 'a>> {
    parent.children().find(|c| {
        c.is_element()
            && c.tag_name().name() == local_name
            && (c.tag_name().namespace() == Some(ns) || c.tag_name().namespace().is_none())
    })
}

/// Recursively collect `assign_value` annotations into a map of dotted paths.
fn collect_xml_assignments(
    element: &ElementDef,
    current_path: &str,
    out: &mut HashMap<String, String>,
) {
    for child in &element.children {
        let path = if current_path.is_empty() {
            child.name.clone()
        } else {
            format!("{current_path}.{}", child.name)
        };

        if let Some(ref assign) = child.assign_value {
            out.insert(path.clone(), assign.clone());
        }

        // Recurse into nested complex types
        collect_xml_assignments(child, &path, out);
    }
}

// ─── Payload validation ─────────────────────────────────────────────────────

/// Validate an XML element node against its schema definition and extract
/// `assign_value` variables.
fn validate_element(
    xml_node: &Node,
    schema_def: &ElementDef,
    vars: &mut HashMap<String, Value>,
) -> Result<(), XmlSchemaError> {
    for child_def in &schema_def.children {
        // Find matching child elements in the XML
        let matching: Vec<Node> = xml_node
            .children()
            .filter(|c| c.is_element() && c.tag_name().name() == child_def.name)
            .collect();

        if matching.is_empty() && child_def.required {
            return Err(XmlSchemaError::ValidationFailed(format!(
                "required element '{}' is missing",
                child_def.name
            )));
        }

        for xml_child in &matching {
            if child_def.children.is_empty() {
                // Leaf element — validate type and extract value
                let text = xml_child.text().unwrap_or("");
                validate_type(text, child_def)?;

                if let Some(ref var_name) = child_def.assign_value {
                    let value = text_to_json_value(text, child_def.xsd_type.as_deref());
                    vars.insert(var_name.clone(), value);
                }
            } else {
                // Complex element — recurse
                validate_element(xml_child, child_def, vars)?;
            }
        }
    }

    Ok(())
}

/// Validate a text value against the declared XSD type.
fn validate_type(text: &str, element: &ElementDef) -> Result<(), XmlSchemaError> {
    let xsd_type = match &element.xsd_type {
        Some(t) => t.as_str(),
        None => return Ok(()), // No type constraint
    };

    // Strip xs: prefix if present
    let type_local = xsd_type
        .strip_prefix("xs:")
        .or_else(|| xsd_type.strip_prefix("xsd:"))
        .unwrap_or(xsd_type);

    match type_local {
        "string" | "normalizedString" | "token" => Ok(()),
        "integer" | "int" | "long" | "short" | "byte" | "nonNegativeInteger"
        | "positiveInteger" | "negativeInteger" | "nonPositiveInteger" | "unsignedInt"
        | "unsignedLong" | "unsignedShort" | "unsignedByte" => {
            text.trim().parse::<i64>().map_err(|_| {
                XmlSchemaError::ValidationFailed(format!(
                    "element '{}': expected {}, got '{}'",
                    element.name, type_local, text
                ))
            })?;
            Ok(())
        }
        "decimal" | "float" | "double" => {
            text.trim().parse::<f64>().map_err(|_| {
                XmlSchemaError::ValidationFailed(format!(
                    "element '{}': expected {}, got '{}'",
                    element.name, type_local, text
                ))
            })?;
            Ok(())
        }
        "boolean" => {
            let t = text.trim();
            if t == "true" || t == "false" || t == "1" || t == "0" {
                Ok(())
            } else {
                Err(XmlSchemaError::ValidationFailed(format!(
                    "element '{}': expected boolean, got '{}'",
                    element.name, text
                )))
            }
        }
        "date" | "dateTime" | "time" => {
            // Basic format validation - not full ISO 8601
            if text.trim().is_empty() {
                Err(XmlSchemaError::ValidationFailed(format!(
                    "element '{}': expected {}, got empty string",
                    element.name, type_local
                )))
            } else {
                Ok(())
            }
        }
        _ => Ok(()), // Unknown types pass through
    }
}

/// Convert XML text content to a `serde_json::Value` based on XSD type.
fn text_to_json_value(text: &str, xsd_type: Option<&str>) -> Value {
    let type_local = xsd_type
        .and_then(|t| {
            t.strip_prefix("xs:")
                .or_else(|| t.strip_prefix("xsd:"))
                .or(Some(t))
        })
        .unwrap_or("string");

    let trimmed = text.trim();

    match type_local {
        "integer" | "int" | "long" | "short" | "byte" | "nonNegativeInteger"
        | "positiveInteger" | "negativeInteger" | "nonPositiveInteger" | "unsignedInt"
        | "unsignedLong" | "unsignedShort" | "unsignedByte" => {
            if let Ok(i) = trimmed.parse::<i64>() {
                Value::Number(i.into())
            } else {
                Value::String(trimmed.to_string())
            }
        }
        "decimal" | "float" | "double" => {
            if let Ok(f) = trimmed.parse::<f64>() {
                serde_json::Number::from_f64(f)
                    .map(Value::Number)
                    .unwrap_or_else(|| Value::String(trimmed.to_string()))
            } else {
                Value::String(trimmed.to_string())
            }
        }
        "boolean" => match trimmed {
            "true" | "1" => Value::Bool(true),
            "false" | "0" => Value::Bool(false),
            _ => Value::String(trimmed.to_string()),
        },
        _ => Value::String(trimmed.to_string()),
    }
}

/// Extract a value from an XML document using a simple dotted path
/// (e.g. `"order.customer.email"`).
///
/// This is used by the authentication module for XML path-based secret lookup.
pub fn extract_xml_path(xml_str: &str, path: &str) -> Option<String> {
    let doc = Document::parse(xml_str).ok()?;
    let mut current = doc.root_element();

    for segment in path.split('.') {
        let child = current
            .children()
            .find(|c| c.is_element() && c.tag_name().name() == segment)?;
        current = child;
    }

    current.text().map(|s| s.trim().to_string())
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── Helper schemas ──────────────────────────────────────────────────

    fn simple_xsd() -> &'static str {
        r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                    xmlns:whd="https://webhookd.dev/schema">
            <xs:element name="product">
                <xs:complexType>
                    <xs:sequence>
                        <xs:element name="productId" type="xs:integer"
                                    whd:assign_value="my-product-id"/>
                        <xs:element name="name" type="xs:string"
                                    whd:assign_value="product-name"
                                    minOccurs="0"/>
                    </xs:sequence>
                </xs:complexType>
            </xs:element>
        </xs:schema>"#
    }

    // ── compile ─────────────────────────────────────────────────────────

    #[test]
    fn compile_valid_xsd() {
        let schema = XmlInputSchema::compile(simple_xsd()).unwrap();
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
    fn compile_invalid_xml_is_error() {
        let err = XmlInputSchema::compile("not xml at all").unwrap_err();
        assert!(matches!(err, XmlSchemaError::InvalidSchemaXml(_)));
    }

    #[test]
    fn compile_missing_root_element_is_error() {
        let xsd = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
        </xs:schema>"#;
        let err = XmlInputSchema::compile(xsd).unwrap_err();
        assert!(matches!(err, XmlSchemaError::SchemaStructure(_)));
    }

    #[test]
    fn compile_element_without_name_is_error() {
        let xsd = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
            <xs:element>
                <xs:complexType>
                    <xs:sequence>
                        <xs:element name="id" type="xs:integer"/>
                    </xs:sequence>
                </xs:complexType>
            </xs:element>
        </xs:schema>"#;
        let err = XmlInputSchema::compile(xsd).unwrap_err();
        assert!(matches!(err, XmlSchemaError::SchemaStructure(_)));
    }

    #[test]
    fn compile_schema_without_assign_value() {
        let xsd = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
            <xs:element name="data">
                <xs:complexType>
                    <xs:sequence>
                        <xs:element name="id" type="xs:integer"/>
                    </xs:sequence>
                </xs:complexType>
            </xs:element>
        </xs:schema>"#;
        let schema = XmlInputSchema::compile(xsd).unwrap();
        assert!(schema.assignments().is_empty());
    }

    // ── validate_and_extract ────────────────────────────────────────────

    #[test]
    fn extract_values_from_valid_payload() {
        let schema = XmlInputSchema::compile(simple_xsd()).unwrap();
        let payload = "<product><productId>42</productId><name>Widget</name></product>";
        let vars = schema.validate_and_extract(payload).unwrap();

        assert_eq!(vars.get("my-product-id"), Some(&json!(42)));
        assert_eq!(vars.get("product-name"), Some(&json!("Widget")));
    }

    #[test]
    fn extract_only_present_optional_elements() {
        let schema = XmlInputSchema::compile(simple_xsd()).unwrap();
        // name has minOccurs="0", so omit it
        let payload = "<product><productId>7</productId></product>";
        let vars = schema.validate_and_extract(payload).unwrap();

        assert_eq!(vars.get("my-product-id"), Some(&json!(7)));
        assert!(
            !vars.contains_key("product-name"),
            "absent optional elements should not appear in vars"
        );
    }

    #[test]
    fn validation_fails_for_missing_required_element() {
        let schema = XmlInputSchema::compile(simple_xsd()).unwrap();
        // productId is required (default minOccurs=1)
        let payload = "<product><name>Widget</name></product>";
        let err = schema.validate_and_extract(payload).unwrap_err();
        assert!(matches!(err, XmlSchemaError::ValidationFailed(_)));
        assert!(err.to_string().contains("productId"));
    }

    #[test]
    fn validation_fails_for_wrong_type() {
        let schema = XmlInputSchema::compile(simple_xsd()).unwrap();
        let payload = "<product><productId>not-an-integer</productId></product>";
        let err = schema.validate_and_extract(payload).unwrap_err();
        assert!(matches!(err, XmlSchemaError::ValidationFailed(_)));
    }

    #[test]
    fn invalid_payload_xml_is_error() {
        let schema = XmlInputSchema::compile(simple_xsd()).unwrap();
        let err = schema.validate_and_extract("not xml").unwrap_err();
        assert!(matches!(err, XmlSchemaError::InvalidPayloadXml(_)));
    }

    #[test]
    fn wrong_root_element_name_is_error() {
        let schema = XmlInputSchema::compile(simple_xsd()).unwrap();
        let payload = "<order><productId>1</productId></order>";
        let err = schema.validate_and_extract(payload).unwrap_err();
        assert!(matches!(err, XmlSchemaError::ValidationFailed(_)));
        assert!(err.to_string().contains("expected root element"));
    }

    // ── nested complex types ────────────────────────────────────────────

    #[test]
    fn extract_from_nested_complex_type() {
        let xsd = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                                xmlns:whd="https://webhookd.dev/schema">
            <xs:element name="order">
                <xs:complexType>
                    <xs:sequence>
                        <xs:element name="id" type="xs:integer"
                                    whd:assign_value="order-id"/>
                        <xs:element name="customer">
                            <xs:complexType>
                                <xs:sequence>
                                    <xs:element name="email" type="xs:string"
                                                whd:assign_value="customer-email"/>
                                    <xs:element name="name" type="xs:string"
                                                whd:assign_value="customer-name"/>
                                </xs:sequence>
                            </xs:complexType>
                        </xs:element>
                    </xs:sequence>
                </xs:complexType>
            </xs:element>
        </xs:schema>"#;

        let schema = XmlInputSchema::compile(xsd).unwrap();
        assert_eq!(schema.assignments().len(), 3);
        assert_eq!(
            schema.assignments().get("id"),
            Some(&"order-id".to_string())
        );
        assert_eq!(
            schema.assignments().get("customer.email"),
            Some(&"customer-email".to_string())
        );
        assert_eq!(
            schema.assignments().get("customer.name"),
            Some(&"customer-name".to_string())
        );

        let payload = r#"<order>
            <id>99</id>
            <customer>
                <email>alice@example.com</email>
                <name>Alice</name>
            </customer>
        </order>"#;
        let vars = schema.validate_and_extract(payload).unwrap();
        assert_eq!(vars.get("order-id"), Some(&json!(99)));
        assert_eq!(
            vars.get("customer-email"),
            Some(&json!("alice@example.com"))
        );
        assert_eq!(vars.get("customer-name"), Some(&json!("Alice")));
    }

    // ── type validation ─────────────────────────────────────────────────

    #[test]
    fn validate_integer_type() {
        let xsd = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
            <xs:element name="data">
                <xs:complexType>
                    <xs:sequence>
                        <xs:element name="count" type="xs:integer"/>
                    </xs:sequence>
                </xs:complexType>
            </xs:element>
        </xs:schema>"#;

        let schema = XmlInputSchema::compile(xsd).unwrap();

        // Valid integer
        assert!(schema
            .validate_and_extract("<data><count>42</count></data>")
            .is_ok());

        // Invalid integer
        assert!(schema
            .validate_and_extract("<data><count>abc</count></data>")
            .is_err());
    }

    #[test]
    fn validate_decimal_type() {
        let xsd = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                                xmlns:whd="https://webhookd.dev/schema">
            <xs:element name="data">
                <xs:complexType>
                    <xs:sequence>
                        <xs:element name="price" type="xs:decimal"
                                    whd:assign_value="item-price"/>
                    </xs:sequence>
                </xs:complexType>
            </xs:element>
        </xs:schema>"#;

        let schema = XmlInputSchema::compile(xsd).unwrap();
        let vars = schema
            .validate_and_extract("<data><price>19.99</price></data>")
            .unwrap();
        assert_eq!(vars.get("item-price"), Some(&json!(19.99)));
    }

    #[test]
    fn validate_boolean_type() {
        let xsd = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                                xmlns:whd="https://webhookd.dev/schema">
            <xs:element name="data">
                <xs:complexType>
                    <xs:sequence>
                        <xs:element name="active" type="xs:boolean"
                                    whd:assign_value="is-active"/>
                    </xs:sequence>
                </xs:complexType>
            </xs:element>
        </xs:schema>"#;

        let schema = XmlInputSchema::compile(xsd).unwrap();

        let vars = schema
            .validate_and_extract("<data><active>true</active></data>")
            .unwrap();
        assert_eq!(vars.get("is-active"), Some(&json!(true)));

        let vars = schema
            .validate_and_extract("<data><active>false</active></data>")
            .unwrap();
        assert_eq!(vars.get("is-active"), Some(&json!(false)));

        // "1" is also valid boolean in XSD
        let vars = schema
            .validate_and_extract("<data><active>1</active></data>")
            .unwrap();
        assert_eq!(vars.get("is-active"), Some(&json!(true)));

        // Invalid boolean
        assert!(schema
            .validate_and_extract("<data><active>maybe</active></data>")
            .is_err());
    }

    #[test]
    fn validate_string_type_accepts_anything() {
        let xsd = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                                xmlns:whd="https://webhookd.dev/schema">
            <xs:element name="data">
                <xs:complexType>
                    <xs:sequence>
                        <xs:element name="msg" type="xs:string"
                                    whd:assign_value="message"/>
                    </xs:sequence>
                </xs:complexType>
            </xs:element>
        </xs:schema>"#;

        let schema = XmlInputSchema::compile(xsd).unwrap();
        let vars = schema
            .validate_and_extract("<data><msg>Hello World!</msg></data>")
            .unwrap();
        assert_eq!(vars.get("message"), Some(&json!("Hello World!")));
    }

    #[test]
    fn validate_float_type() {
        let xsd = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                                xmlns:whd="https://webhookd.dev/schema">
            <xs:element name="data">
                <xs:complexType>
                    <xs:sequence>
                        <xs:element name="value" type="xs:float"
                                    whd:assign_value="val"/>
                    </xs:sequence>
                </xs:complexType>
            </xs:element>
        </xs:schema>"#;

        let schema = XmlInputSchema::compile(xsd).unwrap();
        let vars = schema
            .validate_and_extract("<data><value>3.5</value></data>")
            .unwrap();
        assert_eq!(vars.get("val"), Some(&json!(3.5)));

        // Invalid float
        assert!(schema
            .validate_and_extract("<data><value>not-a-number</value></data>")
            .is_err());
    }

    // ── text_to_json_value ──────────────────────────────────────────────

    #[test]
    fn text_to_json_integer() {
        assert_eq!(text_to_json_value("42", Some("xs:integer")), json!(42));
    }

    #[test]
    fn text_to_json_decimal() {
        assert_eq!(
            text_to_json_value("19.99", Some("xs:decimal")),
            json!(19.99)
        );
    }

    #[test]
    fn text_to_json_boolean_true() {
        assert_eq!(text_to_json_value("true", Some("xs:boolean")), json!(true));
    }

    #[test]
    fn text_to_json_boolean_false() {
        assert_eq!(
            text_to_json_value("false", Some("xs:boolean")),
            json!(false)
        );
    }

    #[test]
    fn text_to_json_boolean_one() {
        assert_eq!(text_to_json_value("1", Some("xs:boolean")), json!(true));
    }

    #[test]
    fn text_to_json_string() {
        assert_eq!(
            text_to_json_value("hello", Some("xs:string")),
            json!("hello")
        );
    }

    #[test]
    fn text_to_json_no_type() {
        assert_eq!(text_to_json_value("hello", None), json!("hello"));
    }

    #[test]
    fn text_to_json_trims_whitespace() {
        assert_eq!(text_to_json_value("  42  ", Some("xs:integer")), json!(42));
    }

    // ── extract_xml_path ────────────────────────────────────────────────

    #[test]
    fn extract_xml_path_simple() {
        let xml = "<root><secret>abc123</secret></root>";
        assert_eq!(extract_xml_path(xml, "secret"), Some("abc123".to_string()));
    }

    #[test]
    fn extract_xml_path_nested() {
        let xml = "<root><auth><token>xyz</token></auth></root>";
        assert_eq!(extract_xml_path(xml, "auth.token"), Some("xyz".to_string()));
    }

    #[test]
    fn extract_xml_path_deeply_nested() {
        let xml = "<root><a><b><c>deep</c></b></a></root>";
        assert_eq!(extract_xml_path(xml, "a.b.c"), Some("deep".to_string()));
    }

    #[test]
    fn extract_xml_path_missing_returns_none() {
        let xml = "<root><a>1</a></root>";
        assert_eq!(extract_xml_path(xml, "b"), None);
    }

    #[test]
    fn extract_xml_path_invalid_xml_returns_none() {
        assert_eq!(extract_xml_path("not xml", "path"), None);
    }

    // ── end-to-end: schema matching config example ──────────────────────

    #[test]
    fn config_example_xsd_works() {
        let xsd = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                                xmlns:whd="https://webhookd.dev/schema">
            <xs:element name="product">
                <xs:complexType>
                    <xs:sequence>
                        <xs:element name="productId" type="xs:integer"
                                    whd:assign_value="my-key"/>
                    </xs:sequence>
                </xs:complexType>
            </xs:element>
        </xs:schema>"#;

        let schema = XmlInputSchema::compile(xsd).unwrap();
        assert_eq!(schema.assignments().len(), 1);
        assert_eq!(
            schema.assignments().get("productId"),
            Some(&"my-key".to_string())
        );

        let payload = "<product><productId>123</productId></product>";
        let vars = schema.validate_and_extract(payload).unwrap();
        assert_eq!(vars.get("my-key"), Some(&json!(123)));
    }

    // ── deeply nested with mixed types ──────────────────────────────────

    #[test]
    fn deeply_nested_with_multiple_types() {
        let xsd = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                                xmlns:whd="https://webhookd.dev/schema">
            <xs:element name="invoice">
                <xs:complexType>
                    <xs:sequence>
                        <xs:element name="id" type="xs:integer"
                                    whd:assign_value="invoice_id"/>
                        <xs:element name="amount" type="xs:decimal"
                                    whd:assign_value="total"/>
                        <xs:element name="paid" type="xs:boolean"
                                    whd:assign_value="is_paid"/>
                        <xs:element name="note" type="xs:string"
                                    whd:assign_value="note"
                                    minOccurs="0"/>
                        <xs:element name="sender">
                            <xs:complexType>
                                <xs:sequence>
                                    <xs:element name="company" type="xs:string"
                                                whd:assign_value="sender_company"/>
                                </xs:sequence>
                            </xs:complexType>
                        </xs:element>
                    </xs:sequence>
                </xs:complexType>
            </xs:element>
        </xs:schema>"#;

        let schema = XmlInputSchema::compile(xsd).unwrap();
        assert_eq!(schema.assignments().len(), 5);

        let payload = r#"<invoice>
            <id>1001</id>
            <amount>250.50</amount>
            <paid>true</paid>
            <note>Rush delivery</note>
            <sender>
                <company>Acme Corp</company>
            </sender>
        </invoice>"#;

        let vars = schema.validate_and_extract(payload).unwrap();
        assert_eq!(vars.get("invoice_id"), Some(&json!(1001)));
        assert_eq!(vars.get("total"), Some(&json!(250.50)));
        assert_eq!(vars.get("is_paid"), Some(&json!(true)));
        assert_eq!(vars.get("note"), Some(&json!("Rush delivery")));
        assert_eq!(vars.get("sender_company"), Some(&json!("Acme Corp")));
    }

    #[test]
    fn optional_element_absent_in_nested() {
        let xsd = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                                xmlns:whd="https://webhookd.dev/schema">
            <xs:element name="data">
                <xs:complexType>
                    <xs:sequence>
                        <xs:element name="required_field" type="xs:string"
                                    whd:assign_value="req"/>
                        <xs:element name="optional_field" type="xs:string"
                                    whd:assign_value="opt"
                                    minOccurs="0"/>
                    </xs:sequence>
                </xs:complexType>
            </xs:element>
        </xs:schema>"#;

        let schema = XmlInputSchema::compile(xsd).unwrap();
        let payload = "<data><required_field>hello</required_field></data>";
        let vars = schema.validate_and_extract(payload).unwrap();
        assert_eq!(vars.get("req"), Some(&json!("hello")));
        assert!(!vars.contains_key("opt"));
    }

    // ── validate_type unit tests ────────────────────────────────────────

    #[test]
    fn validate_type_string_always_passes() {
        let elem = ElementDef {
            name: "test".to_string(),
            xsd_type: Some("xs:string".to_string()),
            required: true,
            assign_value: None,
            children: vec![],
        };
        assert!(validate_type("anything", &elem).is_ok());
    }

    #[test]
    fn validate_type_integer_valid() {
        let elem = ElementDef {
            name: "test".to_string(),
            xsd_type: Some("xs:integer".to_string()),
            required: true,
            assign_value: None,
            children: vec![],
        };
        assert!(validate_type("42", &elem).is_ok());
        assert!(validate_type("-5", &elem).is_ok());
        assert!(validate_type("  100  ", &elem).is_ok());
    }

    #[test]
    fn validate_type_integer_invalid() {
        let elem = ElementDef {
            name: "test".to_string(),
            xsd_type: Some("xs:integer".to_string()),
            required: true,
            assign_value: None,
            children: vec![],
        };
        assert!(validate_type("abc", &elem).is_err());
        assert!(validate_type("3.14", &elem).is_err());
    }

    #[test]
    fn validate_type_boolean_valid() {
        let elem = ElementDef {
            name: "test".to_string(),
            xsd_type: Some("xs:boolean".to_string()),
            required: true,
            assign_value: None,
            children: vec![],
        };
        assert!(validate_type("true", &elem).is_ok());
        assert!(validate_type("false", &elem).is_ok());
        assert!(validate_type("1", &elem).is_ok());
        assert!(validate_type("0", &elem).is_ok());
    }

    #[test]
    fn validate_type_boolean_invalid() {
        let elem = ElementDef {
            name: "test".to_string(),
            xsd_type: Some("xs:boolean".to_string()),
            required: true,
            assign_value: None,
            children: vec![],
        };
        assert!(validate_type("yes", &elem).is_err());
        assert!(validate_type("no", &elem).is_err());
    }

    #[test]
    fn validate_type_none_always_passes() {
        let elem = ElementDef {
            name: "test".to_string(),
            xsd_type: None,
            required: true,
            assign_value: None,
            children: vec![],
        };
        assert!(validate_type("anything", &elem).is_ok());
    }

    // ── collect_xml_assignments ─────────────────────────────────────────

    #[test]
    fn collect_assignments_empty() {
        let root = ElementDef {
            name: "root".to_string(),
            xsd_type: None,
            required: true,
            assign_value: None,
            children: vec![],
        };
        let mut out = HashMap::new();
        collect_xml_assignments(&root, "", &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn collect_assignments_flat() {
        let root = ElementDef {
            name: "root".to_string(),
            xsd_type: None,
            required: true,
            assign_value: None,
            children: vec![
                ElementDef {
                    name: "a".to_string(),
                    xsd_type: Some("xs:string".to_string()),
                    required: true,
                    assign_value: Some("var-a".to_string()),
                    children: vec![],
                },
                ElementDef {
                    name: "b".to_string(),
                    xsd_type: Some("xs:integer".to_string()),
                    required: true,
                    assign_value: Some("var-b".to_string()),
                    children: vec![],
                },
            ],
        };
        let mut out = HashMap::new();
        collect_xml_assignments(&root, "", &mut out);
        assert_eq!(out.len(), 2);
        assert_eq!(out.get("a"), Some(&"var-a".to_string()));
        assert_eq!(out.get("b"), Some(&"var-b".to_string()));
    }

    #[test]
    fn collect_assignments_nested() {
        let root = ElementDef {
            name: "root".to_string(),
            xsd_type: None,
            required: true,
            assign_value: None,
            children: vec![ElementDef {
                name: "outer".to_string(),
                xsd_type: None,
                required: true,
                assign_value: None,
                children: vec![ElementDef {
                    name: "inner".to_string(),
                    xsd_type: Some("xs:string".to_string()),
                    required: true,
                    assign_value: Some("deep-var".to_string()),
                    children: vec![],
                }],
            }],
        };
        let mut out = HashMap::new();
        collect_xml_assignments(&root, "", &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out.get("outer.inner"), Some(&"deep-var".to_string()));
    }
}
