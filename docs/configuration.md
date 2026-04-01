# Configuration

`webhookd` is configured through a TOML file.  A fully-documented example is
provided in [`config.example.toml`](config.example.toml).

## Quick Start

Create a file called `config.toml`:

```toml
port = 8080

[[services]]
endpoint = "/webhook/deploy"
exec = "/opt/webhookd/scripts/deploy.sh"
```

Run the server:

```sh
webhookd config.toml
```

## Reference

### Top-Level Keys

| Key          | Required | Default   | Description |
|--------------|----------|-----------|-------------|
| `port`       | **yes**  | —         | TCP port for the HTTP server. Can also be set via `-p` / `--port`. |
| `hot-reload` | no       | `false`   | Restart the server when the config file changes. Can also be set via `-R` / `--hot-reload`. |
| `log`        | no       | `"info"`  | Log level. One of `debug`, `info`, `warn`, `error`. |

### `[[services]]`

Each `[[services]]` block defines one webhook endpoint.  At least one service
must be configured.

| Key           | Required | Default | Description |
|---------------|----------|---------|-------------|
| `endpoint`    | **yes**  | —       | URL path, e.g. `"/webhook/api1"`. |
| `mimetype`    | no       | —       | Forces MIME validation (`"json"` or `"xml"`). See note below. |
| `input`       | no       | —       | Inline input schema (JSON Schema for JSON, XSD for XML). |
| `output`      | no       | —       | Inline output template (Jinja2-style). |
| `input-file`  | no       | —       | Path to a file with the input schema. Overrides `input`. |
| `output-file` | no       | —       | Path to a file with the output template. Overrides `output`. |
| `exec`        | one of ¹ | —       | Script path to execute. |
| `proxy`       | one of ¹ | —       | URL to proxy requests to. |

> ¹ Exactly one of `exec` or `proxy` must be set per service.

#### MIME Type Note

When an `authenticate` block uses `json` or `xml` as its lookup method, the
MIME type is inferred automatically.  Setting `mimetype` to a *different* value
than what `authenticate` implies is a configuration error.  Only explicitly set
`mimetype` when no `authenticate` block with `json` or `xml` lookup is used, or
ensure it matches the authenticate lookup method.

### `[[services.authenticate]]`

Optional.  When present, requests must carry a secret.

| Key              | Required | Default | Description |
|------------------|----------|---------|-------------|
| `json`           | at most one ² | —  | jq-style path to the secret in a JSON body. |
| `xml`            | at most one ² | —  | Dotted element path to the secret in an XML body (e.g. `"auth.token"`). |
| `http-header`    | at most one ² | `"X-Webhook-Secret"` | HTTP header carrying the secret. |
| `secret`         | one of ³ | —       | Static secret string. |
| `secret-env-var` | one of ³ | —       | Env-var name holding the secret (takes precedence). |

> ² At most one lookup method (`json`, `xml`, `http-header`) may be specified.
> If none is set, `http-header` with the default value `"X-Webhook-Secret"` is
> used.
>
> ³ At least one of `secret` or `secret-env-var` must be provided.  If both are
> set, `secret-env-var` takes precedence at runtime.

## Schema Validation & Variable Extraction

webhookd supports validating incoming payloads against schemas and extracting
values into template variables.  Both **JSON Schema** (for JSON payloads) and
**XSD** (for XML payloads) are supported.

The schema type is auto-detected from the content: schemas starting with `{`
are treated as JSON Schema; schemas starting with `<` are treated as XSD.
You can also set `mimetype = "json"` or `mimetype = "xml"` explicitly.

---

### JSON Schema with `assign_value`

webhookd extends standard [JSON Schema](https://json-schema.org/) with a
custom keyword: **`assign_value`**.  When a property definition includes
`"assign_value": "some-name"`, the value of that property in incoming
payloads is captured as a template variable.

#### How it works

1. Define your JSON Schema as the `input` field (or load from `input-file`)
2. Add `"assign_value": "var-name"` to any property you want to capture
3. Use `{{ var-name }}` in the `output` template to interpolate the value

#### Example

**Config:**

```toml
port = 8080

[[services]]
endpoint = "/webhook/order"
exec = "/opt/scripts/process-order.sh"
input = """
{
  "type": "object",
  "properties": {
    "orderId": {
      "type": "integer",
      "assign_value": "order_id"
    },
    "customer": {
      "type": "object",
      "properties": {
        "email": {
          "type": "string",
          "assign_value": "customer_email"
        }
      }
    },
    "total": {
      "type": "number",
      "assign_value": "order_total"
    }
  },
  "required": ["orderId"]
}
"""
output = """
Order #{{ order_id }} from {{ customer_email }} for ${{ order_total }}
"""
```

**Incoming request:**

```json
{
  "orderId": 12345,
  "customer": { "email": "alice@example.com" },
  "total": 99.99
}
```

**What happens:**

1. The payload is validated against the JSON Schema
2. Variables are extracted: `order_id=12345`, `customer_email="alice@example.com"`,
   `order_total=99.99`
3. The output template renders to:
   `Order #12345 from alice@example.com for $99.99`
4. The script `/opt/scripts/process-order.sh` receives:
   - `WEBHOOKD_BODY` — the raw JSON body
   - `WEBHOOKD_OUTPUT` — the rendered template

#### Supported `assign_value` locations (JSON)

| Location | Example | Variable |
|----------|---------|----------|
| Top-level property | `"id": {"type": "integer", "assign_value": "my_id"}` | `my_id` |
| Nested property | `"order": {"properties": {"id": {"assign_value": "oid"}}}` | `oid` |
| Any JSON type | strings, integers, numbers, booleans, null | Value is passed as-is |

---

### XSD (XML Schema) with `whd:assign_value`

For XML payloads, webhookd validates against XSD schemas extended with a
custom namespace attribute: **`whd:assign_value`** (namespace
`https://webhookd.dev/schema`).  Element values with this attribute are
captured as template variables, just like `assign_value` for JSON.

#### How it works

1. Define your XSD schema as the `input` field (or load from `input-file`)
2. Declare the webhookd namespace: `xmlns:whd="https://webhookd.dev/schema"`
3. Add `whd:assign_value="var-name"` to any `xs:element` you want to capture
4. Use `{{ var-name }}` in the `output` template to interpolate the value

#### Example

**Config:**

```toml
port = 8080

[[services]]
endpoint = "/webhook/xml-order"
mimetype = "xml"
exec = "/opt/scripts/process-xml-order.sh"
input = """
<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
           xmlns:whd="https://webhookd.dev/schema">
  <xs:element name="order">
    <xs:complexType>
      <xs:sequence>
        <xs:element name="orderId" type="xs:integer"
                    whd:assign_value="order_id"/>
        <xs:element name="customer">
          <xs:complexType>
            <xs:sequence>
              <xs:element name="email" type="xs:string"
                          whd:assign_value="customer_email"/>
            </xs:sequence>
          </xs:complexType>
        </xs:element>
        <xs:element name="total" type="xs:decimal"
                    whd:assign_value="order_total"/>
        <xs:element name="note" type="xs:string"
                    whd:assign_value="note"
                    minOccurs="0"/>
      </xs:sequence>
    </xs:complexType>
  </xs:element>
</xs:schema>
"""
output = """
Order #{{ order_id }} from {{ customer_email }} for ${{ order_total }}
"""
```

**Incoming XML request:**

```xml
<order>
  <orderId>12345</orderId>
  <customer>
    <email>alice@example.com</email>
  </customer>
  <total>99.99</total>
</order>
```

**What happens:**

1. The XML payload is validated against the XSD schema
2. Variables are extracted: `order_id=12345`, `customer_email="alice@example.com"`,
   `order_total=99.99`
3. The output template renders to:
   `Order #12345 from alice@example.com for $99.99`
4. The script receives `WEBHOOKD_BODY` (raw XML) and `WEBHOOKD_OUTPUT`
   (rendered template)

#### Supported XSD types

| XSD Type | Rust/JSON Type | Example |
|----------|---------------|---------|
| `xs:string` | String | `"hello"` |
| `xs:integer`, `xs:int`, `xs:long` | Integer | `42` |
| `xs:decimal`, `xs:float`, `xs:double` | Float | `19.99` |
| `xs:boolean` | Boolean | `true`, `false`, `1`, `0` |
| `xs:date`, `xs:dateTime` | String | `"2024-01-15"` |

#### Supported `whd:assign_value` locations (XSD)

| Location | Example | Variable |
|----------|---------|----------|
| Simple element | `<xs:element name="id" type="xs:integer" whd:assign_value="my_id"/>` | `my_id` |
| Nested element | Element inside a `xs:complexType` / `xs:sequence` | Same |
| Optional element | `minOccurs="0"` — only extracted if present in payload | Same |

#### Required vs optional elements

- By default, all elements are **required** (`minOccurs="1"`)
- Set `minOccurs="0"` to make an element optional
- Missing optional elements are simply not included in the variable map

#### XSD namespace declaration

The `whd:assign_value` attribute requires the webhookd namespace to be
declared on the `xs:schema` root element:

```xml
<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
           xmlns:whd="https://webhookd.dev/schema">
```

---

### Variable naming

- Use underscores (`_`) in variable names for straightforward template access:
  `{{ my_var }}`
- Hyphens work but require bracket notation in templates:
  `{{ vars["my-var"] }}`

### Template syntax

Output templates use [Jinja2 syntax](https://jinja.palletsprojects.com/)
via MiniJinja.  Supported features include:

- Variable interpolation: `{{ variable }}`
- Conditionals: `{% if x %}...{% endif %}`
- Loops: `{% for item in items %}...{% endfor %}`
- Filters: `{{ name | upper }}`

## XML Authentication

When using XML payloads, you can authenticate by specifying an XML element
path in the `[[services.authenticate]]` block:

```toml
[[services.authenticate]]
xml = "auth.token"
secret = "my-secret"
```

This extracts the value at `<root><auth><token>...</token></auth></root>`
and compares it to the expected secret.  The path uses dotted notation
to traverse nested elements.

## Script Environment

When `exec` is used, the script receives these environment variables:

| Variable          | Description                                    |
|-------------------|------------------------------------------------|
| `WEBHOOKD_BODY`   | The raw request body                           |
| `WEBHOOKD_OUTPUT` | The rendered output template (if configured)   |

The script's **stdout** becomes the HTTP response body.  A non-zero exit
code results in HTTP 500.
