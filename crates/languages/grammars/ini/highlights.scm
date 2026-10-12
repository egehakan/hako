; Keys that precede the first section header, as in .npmrc and .editorconfig, land in an error
; node instead of a `setting`, so the bundled query never reaches them.
(ERROR
  (setting_name) @property
  .
  "=")
