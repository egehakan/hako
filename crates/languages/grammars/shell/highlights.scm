; Variables
;----------

; The sigil belongs to the variable; the bundled query captures it as an operator.
(simple_expansion
  "$" @property)
(special_variable_name) @property
(string
  (expansion
    ["${" "}"] @string))

(variable_assignment
  "=" @operator)
(variable_assignment
  value: (word) @string)
(array
  (word) @string)
(for_statement
  value: (word) @string)

(number) @number

; Commands
;---------

((command_name) @function.builtin
  (#match? @function.builtin
    "^(alias|bg|bind|builtin|caller|cd|command|compgen|complete|dirs|disown|echo|enable|eval|exec|exit|false|fc|fg|getopts|hash|help|history|jobs|kill|let|logout|mapfile|popd|printf|pushd|pwd|read|readarray|set|shift|shopt|source|suspend|test|times|trap|true|type|ulimit|umask|unalias|wait)$"))
((command_name) @keyword
  (#match? @keyword "^(break|continue|return)$"))

(declaration_command
  ["declare" "export" "local" "readonly" "typeset"] @keyword)

(command
  argument: (word) @string)
((command
  argument: (word) @constant)
  (#match? @constant "^-"))

; Operators
;----------

; Separators between commands, as opposed to the operators of a test expression.
(pipeline
  ["|" "|&"] @punctuation.delimiter)
(list
  ["&&" "||"] @punctuation.delimiter)
(file_redirect
  ["<" ">" ">>" ">&" "<&" "&>" "&>>" ">|"] @punctuation.delimiter)
(file_redirect
  descriptor: (file_descriptor) @punctuation.delimiter)
(file_redirect
  destination: (word) @string)

(test_operator) @operator
(binary_expression
  operator: ["=" "==" "!=" "=~" "&&" "||" "<" ">"] @operator)
(unary_expression
  operator: "!" @operator)
