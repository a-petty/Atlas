; Go declarations and explicit references use the Go grammar.
(function_declaration name: (identifier) @definition.function)
(method_declaration name: (field_identifier) @definition.method)
(type_spec name: (type_identifier) @definition.type)
(type_alias name: (type_identifier) @definition.type)
(var_spec name: (identifier) @definition.variable)
(const_spec name: (identifier) @definition.variable)
(field_declaration name: (field_identifier) @definition.variable)
(call_expression function: (identifier) @reference.call)
(call_expression function: (selector_expression field: (field_identifier) @reference.method))
(type_identifier) @reference.type
(method_spec name: (field_identifier) @definition.method)
