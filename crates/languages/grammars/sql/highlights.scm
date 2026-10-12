; The bundled query tests numeric literals against Lua patterns, which never match as regexes,
; so every number would otherwise keep the generic literal (string) highlight.
((literal) @number
  (#match? @number "^[-+]?[0-9]+([.][0-9]+)?$"))
