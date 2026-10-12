; The bundled query captures a doctype as a single constant and attribute values without their
; quotes.
(doctype
  ["<!" ">"] @punctuation.bracket)
(doctype
  "doctype" @tag)

(quoted_attribute_value) @string

(entity) @constant.character.entity
