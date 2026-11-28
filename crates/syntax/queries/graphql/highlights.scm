; Zuse-maintained GraphQL highlights. `tree-sitter-graphql` 0.3 gates its
; bundled queries behind cfgs its build never sets, so this follows the
; grammar's node types directly.

(comment) @comment
(description (string_value) @comment)
(string_value) @string
(int_value) @number
(float_value) @number
(boolean_value) @boolean
(null_value) @constant
(enum_value (name) @constant)

(operation_type) @keyword
(operation_definition (name) @function)
(fragment_definition (fragment_name (name) @function))
(fragment_spread (fragment_name (name) @function))
(type_condition (named_type (name) @type))
(named_type (name) @type)

(object_type_definition (name) @type)
(interface_type_definition (name) @type)
(input_object_type_definition (name) @type)
(enum_type_definition (name) @type)
(union_type_definition (name) @type)
(scalar_type_definition (name) @type)
(object_type_extension (name) @type)
(interface_type_extension (name) @type)
(input_object_type_extension (name) @type)
(enum_type_extension (name) @type)
(union_type_extension (name) @type)
(scalar_type_extension (name) @type)
(enum_value_definition (enum_value (name) @constant))

(field (name) @property)
(field (alias (name) @property))
(field_definition (name) @property)
(object_field (name) @property)
(argument (name) @variable.parameter)
(input_value_definition (name) @variable.parameter)
(variable) @variable

(directive "@" @attribute (name) @attribute)
(directive_definition "@" @attribute (name) @attribute)
(directive_location) @constant

[
  "query" "mutation" "subscription" "fragment" "on" "type" "interface" "union"
  "enum" "input" "scalar" "schema" "extend" "directive" "implements" "repeatable"
] @keyword

["!" "=" "|" "&" "..."] @operator
["(" ")" "[" "]" "{" "}" ":" "$"] @punctuation
(comma) @punctuation
