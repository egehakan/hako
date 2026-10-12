(escape_sequence) @string.escape

(block_scalar
  ["|" ">"] @keyword.operator)

(yaml_directive
  (yaml_version) @number)

; Document markers separate documents; the bundled query groups them with anchors and aliases.
[
  "---"
  "..."
] @punctuation.delimiter
