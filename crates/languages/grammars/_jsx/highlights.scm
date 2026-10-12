; JSX patterns shared by the JavaScript and TSX grammars. The bundled JavaScript query has none,
; and the bundled TSX query colors JSX text as a string and only the last segment of a dotted
; component name.

(jsx_opening_element
  name: (identifier) @tag)
(jsx_closing_element
  name: (identifier) @tag)
(jsx_self_closing_element
  name: (identifier) @tag)

((jsx_opening_element
  name: (identifier) @constructor)
  (#match? @constructor "^[A-Z]"))
((jsx_closing_element
  name: (identifier) @constructor)
  (#match? @constructor "^[A-Z]"))
((jsx_self_closing_element
  name: (identifier) @constructor)
  (#match? @constructor "^[A-Z]"))

(jsx_opening_element
  name: (member_expression) @constructor)
(jsx_closing_element
  name: (member_expression) @constructor)
(jsx_self_closing_element
  name: (member_expression) @constructor)
(jsx_opening_element
  name: (member_expression
    [(identifier) (property_identifier) "."] @constructor))
(jsx_closing_element
  name: (member_expression
    [(identifier) (property_identifier) "."] @constructor))
(jsx_self_closing_element
  name: (member_expression
    [(identifier) (property_identifier) "."] @constructor))

(jsx_attribute
  (property_identifier) @attribute)

(jsx_expression
  ["{" "}"] @punctuation.special)

(jsx_text) @none

(jsx_opening_element
  ["<" ">"] @punctuation.bracket)
(jsx_closing_element
  ["</" ">"] @punctuation.bracket)
(jsx_self_closing_element
  ["<" "/>"] @punctuation.bracket)
