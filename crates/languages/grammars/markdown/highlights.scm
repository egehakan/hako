; The bundled query gives heading markers, list markers, quote markers and thematic breaks one
; shared capture.
[
  (atx_h1_marker)
  (atx_h2_marker)
  (atx_h3_marker)
  (atx_h4_marker)
  (atx_h5_marker)
  (atx_h6_marker)
  (setext_h1_underline)
  (setext_h2_underline)
] @punctuation.special.heading

[
  (list_marker_plus)
  (list_marker_minus)
  (list_marker_star)
  (list_marker_dot)
  (list_marker_parenthesis)
] @punctuation.special.list

(thematic_break) @punctuation.delimiter

(block_quote) @markup.quote
(block_quote_marker) @markup.quote
(block_quote
  (paragraph
    (inline
      (block_continuation) @markup.quote)))

; Code is not a string literal; only its delimiters are markup.
[
  (info_string)
  (indented_code_block)
] @none
