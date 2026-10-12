; The bundled query captures every string after the key pattern, so keys would otherwise be
; colored as ordinary strings.
(pair
  key: (string) @string.special.key)

(array
  [(true) (false) (null)] @constant.builtin.array)
