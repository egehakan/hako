; Variables
;----------

; The sigil belongs to the variable; the bundled query captures it as an operator.
(variable_name
  "$" @variable)
((variable_name
  "$" @variable.builtin
  (name) @_name)
  (#eq? @_name "this"))

; Types and namespaces
;---------------------

(class_declaration
  name: (name) @type.class)
(interface_declaration
  name: (name) @type.interface)
(trait_declaration
  name: (name) @type.class)
(enum_declaration
  name: (name) @type.enum)
(base_clause
  [
    (name) @type
    (qualified_name
      (name) @type)
  ])
(class_interface_clause
  [
    (name) @type
    (qualified_name
      (name) @type)
  ])

(namespace_definition
  name: (namespace_name
    "\\" @module))
(qualified_name
  prefix: (namespace_name
    (name) @module.qualifier))
(object_creation_expression
  (qualified_name
    prefix: "\\" @constructor))

; Functions
;----------

((method_declaration
  name: (name) @function.builtin)
  (#match? @function.builtin "^__[a-zA-Z]+$"))

[
  "echo"
  "print"
] @function.builtin

; Functions the language itself provides, by the prefixes and names most code relies on.
((function_call_expression
  function: (name) @function.builtin)
  (#match? @function.builtin
    "^((array|str|is|preg|json|mb|ctype|file|curl|date|ob|session|iterator|spl|password|random|filter)_[a-z0-9_]+|isset|empty|unset|count|sizeof|in_array|implode|explode|compact|extract|range|usort|uasort|uksort|ksort|krsort|asort|arsort|sort|rsort|min|max|abs|ceil|floor|round|intval|floatval|boolval|strval|settype|gettype|strlen|strpos|strrpos|stripos|strstr|strtolower|strtoupper|strtotime|strrev|substr|trim|ltrim|rtrim|ucfirst|ucwords|lcfirst|nl2br|sprintf|printf|vsprintf|number_format|htmlspecialchars|htmlentities|addslashes|stripslashes|md5|sha1|hash|base64_encode|base64_decode|urlencode|urldecode|rawurlencode|http_build_query|parse_url|serialize|unserialize|var_dump|var_export|print_r|date|time|mktime|microtime|sleep|usleep|uniqid|rand|mt_rand|fopen|fclose|fread|fwrite|fgets|unlink|mkdir|rmdir|basename|dirname|realpath|pathinfo|glob|header|setcookie|define|defined|constant|function_exists|class_exists|method_exists|property_exists|get_class|call_user_func|call_user_func_array|func_get_args|error_log|trigger_error|die)$"))

; Operators
;----------

[
  "="
  "+="
  "-="
  "*="
  "/="
  ".="
  "??="
  "+"
  "-"
  "*"
  "/"
  "%"
  "**"
  "++"
  "--"
  "=="
  "==="
  "!="
  "!=="
  "<>"
  "<"
  "<="
  ">"
  ">="
  "<=>"
  "&&"
  "||"
  "!"
  "&"
  "|"
  "^"
  "~"
  "<<"
  ">>"
] @operator
