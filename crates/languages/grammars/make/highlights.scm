(comment) @comment

[
  "("
  ")"
  "{"
  "}"
] @punctuation.bracket

[
  ":"
  "&:"
  "::"
  "|"
  ";"
  ","
] @punctuation.delimiter

[
  "ifeq"
  "ifneq"
  "ifdef"
  "ifndef"
  "else"
  "endif"
  "if"
  "or"
  "and"
  "foreach"
  "define"
  "endef"
  "vpath"
  "undefine"
  "export"
  "unexport"
  "override"
  "private"
  "include"
  "sinclude"
  "-include"
] @keyword

[
  "subst"
  "patsubst"
  "strip"
  "findstring"
  "filter"
  "filter-out"
  "sort"
  "word"
  "words"
  "wordlist"
  "firstword"
  "lastword"
  "dir"
  "notdir"
  "suffix"
  "basename"
  "addsuffix"
  "addprefix"
  "join"
  "wildcard"
  "realpath"
  "abspath"
  "call"
  "eval"
  "file"
  "value"
  "shell"
  "error"
  "warning"
  "info"
] @function.builtin

(variable_assignment
  value: (text) @string)

(define_directive
  value: (raw_text) @string)

(string) @string

(variable_assignment
  name: (word) @property)

(define_directive
  name: (word) @property)

(variable_reference
  (word) @property)

(automatic_variable) @property

(targets
  (word) @function)

; Special targets such as .PHONY configure make rather than name something to build.
(targets
  (word) @keyword
  (#match? @keyword "^[.][A-Z_]+$"))
