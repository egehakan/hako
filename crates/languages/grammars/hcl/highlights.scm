; The bundled query ends with a pattern matching every identifier, which would otherwise take
; precedence over each more specific identifier pattern before it.
(block
  (identifier) @type)
(attribute
  (identifier) @property)
(object_elem
  key: (expression
    (variable_expr
      (identifier) @property)))
(get_attr
  (identifier) @property)
(function_call
  (identifier) @function.call)
