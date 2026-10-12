; Shared by TypeScript and TSX.

; Types
;------

(class_declaration
  name: (type_identifier) @type.class)
(abstract_class_declaration
  name: (type_identifier) @type.class)
(interface_declaration
  name: (type_identifier) @type.interface)
(enum_declaration
  name: (identifier) @type.enum)
(type_alias_declaration
  name: (type_identifier) @type.alias)
(type_parameter
  name: (type_identifier) @type.parameter)

(literal_type
  [(null) (undefined)] @type.builtin)
(predefined_type
  "void" @type.builtin)

(internal_module
  name: (identifier) @module)

(enum_body
  name: (property_identifier) @constant.enum)
(enum_assignment
  name: (property_identifier) @constant.enum)

(required_parameter
  (rest_pattern
    (identifier) @variable.parameter))

; Members
;--------

(method_signature
  name: (property_identifier) @function.method)
(abstract_method_signature
  name: (property_identifier) @function.method)
(public_field_definition
  name: (property_identifier) @function.method
  value: [(function_expression) (arrow_function)])

; Operators and keywords
;-----------------------

(type_parameters
  ["<" ">"] @punctuation.bracket)
(union_type
  "|" @punctuation.delimiter)
(intersection_type
  "&" @punctuation.delimiter)

(optional_parameter
  "?" @keyword.operator)
(property_signature
  "?" @keyword.operator)
(method_signature
  "?" @keyword.operator)
(public_field_definition
  "?" @keyword.operator)

[
  "asserts"
  "infer"
  "is"
  "module"
] @keyword
