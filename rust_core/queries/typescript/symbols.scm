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


; Declaration-only and abstract APIs (including .d.ts files).
(abstract_class_declaration name: (type_identifier) @definition.class)
(function_signature name: (identifier) @definition.function)
(generator_function_declaration name: (identifier) @definition.function)
(method_signature name: (_) @definition.method)
(abstract_method_signature name: (_) @definition.method)
(property_signature name: (_) @definition.variable)
(public_field_definition name: (_) @definition.variable)
