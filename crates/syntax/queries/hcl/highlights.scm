; Zuse-maintained HCL/Terraform highlights. The `tree-sitter-hcl` crate ships
; no query, so this follows the grammar's node types directly.

(comment) @comment

(block (identifier) @keyword)
(block (string_lit (template_literal) @type))
(attribute (identifier) @property)
(object_elem key: (expression (variable_expr (identifier) @property)))
(get_attr (identifier) @property)
(function_call (identifier) @function)
(for_intro (identifier) @variable)
(variable_expr (identifier) @variable)

((variable_expr (identifier) @variable.builtin)
  (#any-of? @variable.builtin "var" "local" "module" "data" "path" "terraform" "each" "count" "self"))

(string_lit) @string
(quoted_template) @string
(heredoc_template) @string
(heredoc_identifier) @label
(heredoc_start) @punctuation
(template_literal) @string
(numeric_lit) @number
(bool_lit) @boolean
(null_lit) @constant

[
  (template_interpolation_start)
  (template_interpolation_end)
  (template_directive_start)
  (template_directive_end)
  (strip_marker)
] @punctuation.special

["for" "in" "if" "else" "endif" "endfor"] @keyword

[
  "!" "!=" "%" "&&" "*" "+" "-" "/" "<" "<=" "==" ">" ">=" "?" "||" "=>" "=" ":"
] @operator

["(" ")" "[" "]" "{" "}" "," "." ".*" "[*]"] @punctuation
(ellipsis) @punctuation
