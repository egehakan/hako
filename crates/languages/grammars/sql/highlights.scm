; The bundled query tests numeric literals against Lua patterns, which never match as regexes,
; so every number would otherwise keep the generic literal (string) highlight.
((literal) @number
  (#match? @number "^[-+]?[0-9]+([.][0-9]+)?$"))

; Both halves of a schema- or table-qualified name
(object_reference
  schema: (identifier) @variable.qualified
  name: (identifier) @variable.qualified)
(field
  (object_reference
    name: (identifier) @variable.qualified)
  name: (identifier) @variable.qualified)

; Double-quoted identifiers
((identifier) @string
  (#match? @string "^\""))

(binary_expression
  operator: ["+" "-" "*" "/" "%"] @operator.arithmetic)
(all_fields
  "*" @operator.arithmetic)
