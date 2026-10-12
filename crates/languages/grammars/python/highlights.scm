; Classes
;--------

(class_definition
  name: (identifier) @type.class)
(class_definition
  superclasses: (argument_list
    [
      (identifier) @type.class
      (attribute) @type.class
      (attribute
        [(identifier) "."] @type.class)
      (keyword_argument
        name: (identifier) @variable
        value: (identifier) @type.class)
    ]))

; Built-in types, wherever they are named
((identifier) @type.builtin
  (#match? @type.builtin
    "^(bool|bytearray|bytes|classmethod|complex|dict|float|frozenset|int|list|object|property|set|slice|staticmethod|str|tuple|type)$"))

; Variables
;----------

((identifier) @variable.magic
  (#match? @variable.magic
    "^__(all|annotations|builtins|class|debug|dict|doc|file|loader|module|name|package|path|qualname|slots|spec)__$"))

(parameters
  [
    (identifier) @variable.parameter
    (typed_parameter
      (identifier) @variable.parameter)
    (default_parameter
      name: (identifier) @variable.parameter)
    (typed_default_parameter
      name: (identifier) @variable.parameter)
    (list_splat_pattern
      (identifier) @variable.parameter)
    (dictionary_splat_pattern
      (identifier) @variable.parameter)
  ])
(lambda_parameters
  [
    (identifier) @variable.parameter
    (default_parameter
      name: (identifier) @variable.parameter)
    (list_splat_pattern
      (identifier) @variable.parameter)
    (dictionary_splat_pattern
      (identifier) @variable.parameter)
  ])

(call
  arguments: (argument_list
    (keyword_argument
      name: (identifier) @variable.argument)))

((identifier) @variable.builtin
  (#match? @variable.builtin "^(self|cls)$"))

; Functions
;----------

; The bundled query captures a whole decorator, arguments included, and tags attributes after
; method calls, so a called attribute would otherwise be colored as a property.
(decorator) @none
(decorator
  "@" @function.decorator)
(decorator
  [
    (identifier) @function.decorator
    (attribute
      (identifier) @function.decorator)
    (call
      function: [
        (identifier) @function.decorator
        (attribute
          (identifier) @function.decorator)
      ])
  ])
((decorator
  (identifier) @function.builtin)
  (#match? @function.builtin "^(classmethod|property|staticmethod)$"))

(attribute
  attribute: (identifier) @property)
(call
  function: (attribute
    attribute: (identifier) @function.method.call))
(call
  function: (identifier) @function.call)

((call
  function: (identifier) @function.builtin)
  (#match? @function.builtin
    "^(abs|all|any|ascii|bin|bool|breakpoint|bytearray|bytes|callable|chr|classmethod|compile|complex|delattr|dict|dir|divmod|enumerate|eval|exec|filter|float|format|frozenset|getattr|globals|hasattr|hash|help|hex|id|input|int|isinstance|issubclass|iter|len|list|locals|map|max|memoryview|min|next|object|oct|open|ord|pow|print|property|range|repr|reversed|round|set|setattr|slice|sorted|staticmethod|str|sum|super|tuple|type|vars|zip|__import__)$"))

((function_definition
  name: (identifier) @function.builtin)
  (#match? @function.builtin "^__[a-z0-9_]+__$"))
((call
  function: (attribute
    attribute: (identifier) @function.builtin))
  (#match? @function.builtin "^__[a-z0-9_]+__$"))

; Built-in exception classes, including where they are called to construct one
((identifier) @type.exception
  (#match? @type.exception
    "^((Arithmetic|Assertion|Attribute|Buffer|BlockingIO|BrokenPipe|ChildProcess|Connection|ConnectionAborted|ConnectionRefused|ConnectionReset|EOF|Environment|FileExists|FileNotFound|FloatingPoint|IO|Import|Indentation|Index|Interrupted|IsADirectory|Key|Lookup|Memory|ModuleNotFound|Name|NotADirectory|NotImplemented|OS|Overflow|Permission|ProcessLookup|Recursion|Reference|Runtime|Syntax|System|Tab|Timeout|Type|UnboundLocal|Unicode|UnicodeDecode|UnicodeEncode|UnicodeTranslate|Value|ZeroDivision)Error|(Bytes|Deprecation|Future|Import|PendingDeprecation|Resource|Runtime|Syntax|Unicode|User)?Warning|SystemExit|StopIteration|StopAsyncIteration|KeyboardInterrupt|GeneratorExit|BaseException|Exception)$"))

; Operators
;----------

[
  "and"
  "in"
  "is"
  "is not"
  "not"
  "not in"
  "or"
] @keyword.operator

(binary_operator
  operator: "@" @operator)

"->" @punctuation.delimiter
(default_parameter
  "=" @punctuation.delimiter)
(parameters
  (list_splat_pattern
    "*" @punctuation.delimiter))
(parameters
  (dictionary_splat_pattern
    "**" @punctuation.delimiter))

; Format strings
;---------------

[
  (type_conversion)
  (format_specifier)
] @keyword.directive
