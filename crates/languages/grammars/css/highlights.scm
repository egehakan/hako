; Selectors
;----------

; The bundled query tags class, id and property names alike, and leaves each selector's leading
; punctuation to the generic delimiter pattern.
(class_selector
  ["." (class_name)] @attribute.class)
(id_selector
  ["#" (id_name)] @attribute.id)
(pseudo_class_selector
  [":" (class_name)] @attribute.pseudo)
(pseudo_element_selector
  ["::" (tag_name)] @attribute.pseudo)
(pseudo_class_selector
  (arguments
    ["(" ")"] @punctuation.special))
(attribute_selector
  ["[" "]"] @punctuation.special)
(attribute_selector
  ["=" "^=" "|=" "~=" "$=" "*="] @punctuation.delimiter)
(universal_selector
  "*" @tag)

(child_selector
  ">" @punctuation.delimiter)
(sibling_selector
  "~" @punctuation.delimiter)
(adjacent_sibling_selector
  "+" @punctuation.delimiter)

; Declarations
;-------------

((property_name) @property.vendored
  (#match? @property.vendored "^-[a-z]+-"))
((property_name) @variable
  (#match? @variable "^--"))

(declaration
  (plain_value) @constant)
((plain_value) @variable
  (#match? @variable "^--"))
((call_expression
  (function_name) @_function
  (arguments
    (plain_value) @variable.parameter))
  (#eq? @_function "url"))

(color_value
  "#" @string.special)

(keyframes_name) @variable
