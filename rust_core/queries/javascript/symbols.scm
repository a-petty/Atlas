; Function declarations
(function_declaration
  name: (identifier) @definition.function)

; Arrow functions assigned to variables
(variable_declarator
  name: (identifier) @definition.function
  value: (arrow_function))

; Class declarations
(class_declaration
  name: (identifier) @definition.class)

; Method definitions
(method_definition
  name: (property_identifier) @definition.method)

; Variable declarations (const, let, var)
(variable_declarator
  name: (identifier) @definition.variable)

; Function calls
(call_expression
  function: [
    (identifier) @reference.call
    (member_expression property: (property_identifier) @reference.call)
  ])

; New expressions
(new_expression
  constructor: (identifier) @reference.class)

