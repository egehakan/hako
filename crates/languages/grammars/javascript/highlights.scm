; inherits: _ecma,_jsx

; Class names are plain identifiers in this grammar, where TypeScript parses type identifiers.
(class_declaration
  name: (identifier) @type.class)
(class
  name: (identifier) @type.class)
(class_heritage
  (identifier) @type)

(field_definition
  property: (property_identifier) @function.method
  value: [(function_expression) (arrow_function)])

; TypeScript wraps each parameter in a node of its own, which its bundled query captures.
(formal_parameters
  [
    (identifier) @variable.parameter
    (assignment_pattern
      left: (identifier) @variable.parameter)
    (rest_pattern
      (identifier) @variable.parameter)
  ])
