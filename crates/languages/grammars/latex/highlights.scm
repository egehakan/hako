[
  (line_comment)
  (block_comment)
  (comment_environment)
] @comment

[
  "{"
  "}"
  "["
  "]"
] @punctuation.bracket

[
  (inline_formula)
  (displayed_equation)
  (math_environment)
] @string

; Commands the grammar knows structurally (\section, \usepackage, \cite, ...) expose their name
; through a `command` field; every other command is a generic one.
(_
  command: _ @keyword)

(generic_command
  command: (command_name) @function)

(new_command_definition
  declaration: (curly_group_command_name
    (command_name) @function))

(begin
  name: (curly_group_text
    (text) @type))

(end
  name: (curly_group_text
    (text) @type))

(title_declaration
  text: (curly_group
    (_) @text.title))

(part
  text: (curly_group
    (_) @text.title))

(chapter
  text: (curly_group
    (_) @text.title))

(section
  text: (curly_group
    (_) @text.title))

(subsection
  text: (curly_group
    (_) @text.title))

(subsubsection
  text: (curly_group
    (_) @text.title))

(paragraph
  text: (curly_group
    (_) @text.title))

(subparagraph
  text: (curly_group
    (_) @text.title))

(label_definition
  name: (curly_group_label
    (label) @text.reference))

(label_reference
  names: (curly_group_label_list
    (label) @text.reference))

(citation
  keys: (curly_group_text_list
    (text) @text.reference))

(curly_group_path
  (path) @string)

(curly_group_path_list
  (path) @string)

(curly_group_uri
  (uri) @text.uri)

[
  (placeholder)
  (argc)
] @number
