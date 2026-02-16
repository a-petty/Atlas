; Include all JavaScript patterns
(function_declaration
  name: (identifier) @definition.function)

(variable_declarator
  name: (identifier) @definition.function
  value: (arrow_function))

(class_declaration
  name: (type_identifier) @definition.class)

(method_definition
  name: (property_identifier) @definition.method)

(variable_declarator
  name: (identifier) @definition.variable)

; TypeScript-specific: Interface declarations
(interface_declaration
  name: (type_identifier) @definition.interface)

; TypeScript-specific: Type alias
(type_alias_declaration
  name: (type_identifier) @definition.type)

; TypeScript-specific: Enum declarations
(enum_declaration
  name: (identifier) @definition.enum)

; Function calls
(call_expression
  function: [
    (identifier) @reference.call
    (member_expression property: (property_identifier) @reference.call)
  ])

; New expressions
(new_expression
  constructor: (identifier) @reference.class)

