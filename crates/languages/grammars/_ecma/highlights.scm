; Shared by JavaScript, JSX, TypeScript and TSX, so every node named here exists in all four
; grammars. The bundled query tags each capitalized identifier as a constructor or type in a late
; pattern, so the declaration and call patterns are repeated here to take precedence again.

; Bindings
;---------

[
  (shorthand_property_identifier)
  (shorthand_property_identifier_pattern)
] @variable

([
  (shorthand_property_identifier)
  (shorthand_property_identifier_pattern)
] @constant
  (#match? @constant "^[A-Z_][A-Z\\d_]+$"))

(import_clause
  (identifier) @variable)
(import_specifier
  name: (identifier) @variable)
(import_specifier
  alias: (identifier) @variable)
(namespace_import
  (identifier) @variable)

(member_expression
  object: (identifier) @variable.object)

((identifier) @variable.builtin
  (#match? @variable.builtin
    "^(arguments|module|exports|console|window|document|globalThis|navigator|localStorage|sessionStorage)$"))

(lexical_declaration
  kind: "const"
  (variable_declarator
    name: [
      (identifier) @variable.constant
      (array_pattern
        (identifier) @variable.constant)
      (object_pattern
        [
          (shorthand_property_identifier_pattern) @variable.constant
          (pair_pattern
            value: (identifier) @variable.constant)
          (object_assignment_pattern
            left: (shorthand_property_identifier_pattern) @variable.constant)
          (rest_pattern
            (identifier) @variable.constant)
        ])
    ]))

(for_in_statement
  kind: "const"
  left: (identifier) @variable.constant)

(arrow_function
  parameter: (identifier) @variable.parameter)

(private_property_identifier) @property

; Functions
;----------

(function_expression
  name: (identifier) @function)
(function_declaration
  name: (identifier) @function)
(generator_function_declaration
  name: (identifier) @function)
(variable_declarator
  name: (identifier) @function
  value: [(function_expression) (arrow_function)])
(assignment_expression
  left: (identifier) @function
  right: [(function_expression) (arrow_function)])

(method_definition
  name: [(property_identifier) (private_property_identifier)] @function.method)
(pair
  key: (property_identifier) @function.method
  value: [(function_expression) (arrow_function)])
(assignment_expression
  left: (member_expression
    property: (property_identifier) @function.method)
  right: [(function_expression) (arrow_function)])

((method_definition
  name: (property_identifier) @keyword)
  (#eq? @keyword "constructor"))

(call_expression
  function: (identifier) @function.call)
(call_expression
  function: (member_expression
    property: [(property_identifier) (private_property_identifier)] @function.method.call))

; Globals that every runtime provides, as opposed to functions the surrounding code defines.
((call_expression
  function: (identifier) @function.builtin)
  (#match? @function.builtin
    "^(parseInt|parseFloat|isNaN|isFinite|setTimeout|setInterval|clearTimeout|clearInterval|queueMicrotask|structuredClone|fetch|encodeURI|encodeURIComponent|decodeURI|decodeURIComponent|requestAnimationFrame|cancelAnimationFrame|atob|btoa|alert|eval)$"))

((call_expression
  function: (member_expression
    object: (identifier) @_object
    property: (property_identifier) @function.method.builtin))
  (#match? @_object
    "^(console|Math|JSON|Object|Array|Number|String|Boolean|Symbol|Promise|Reflect|Date|BigInt|Intl)$"))

; Methods of built-in prototypes, limited to names code rarely gives its own methods.
((call_expression
  function: (member_expression
    property: (property_identifier) @function.method.builtin))
  (#match? @function.method.builtin
    "^(forEach|map|filter|reduce|reduceRight|flatMap|findIndex|findLast|findLastIndex|indexOf|lastIndexOf|includes|some|every|concat|slice|splice|join|unshift|pop|shift|sort|split|then|catch|finally|startsWith|endsWith|padStart|padEnd|trim|trimStart|trimEnd|toLowerCase|toUpperCase|toString|toFixed|toISOString|toLocaleString|toLocaleDateString|toLocaleTimeString|replaceAll|charAt|charCodeAt|substring|localeCompare|hasOwnProperty|addEventListener|removeEventListener|querySelector|querySelectorAll|getElementById|preventDefault|stopPropagation)$"))

(new_expression
  constructor: (identifier) @constructor)
(new_expression
  constructor: (member_expression
    property: (property_identifier) @constructor))

(decorator
  (identifier) @function.decorator)
(decorator
  (member_expression
    property: (property_identifier) @function.decorator))
(decorator
  (call_expression
    function: (identifier) @function.decorator))
(decorator
  (call_expression
    function: (member_expression
      property: (property_identifier) @function.decorator)))

; Literals
;---------

(escape_sequence) @string.escape

(regex
  "/" @string)
(regex_flags) @keyword

; Operators
;----------

"=>" @keyword.function

(ternary_expression
  ["?" ":"] @keyword.operator)

(augmented_assignment_expression
  operator: _ @operator.assignment.compound)
