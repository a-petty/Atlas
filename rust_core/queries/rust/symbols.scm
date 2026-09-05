; === RUST SYMBOL QUERIES ===

; ========== DEFINITIONS ==========

; Functions
(function_item 
  name: (identifier) @def.function)

; Structs
(struct_item 
  name: (type_identifier) @def.struct)

; Enums
(enum_item 
  name: (type_identifier) @def.enum)

; Traits
(trait_item 
  name: (type_identifier) @def.trait)

; Modules
(mod_item 
  name: (identifier) @def.module)

; Constants
(const_item 
  name: (identifier) @def.constant)

; Static variables
(static_item 
  name: (identifier) @def.static)

; Let bindings: let x = 5;
(let_declaration 
  pattern: (identifier) @def.variable)

; Let bindings with destructuring: let (a, b) = ...;
(let_declaration
  pattern: (tuple_pattern
    (identifier) @def.variable))

; Function parameters
(parameters 
  (parameter 
    pattern: (identifier) @def.parameter))

; Struct fields
(field_declaration 
  name: (field_identifier) @def.field)

; Use declarations - simple form
(use_declaration 
  argument: (identifier) @def.import)

; Use declarations - scoped form: use std::collections::HashMap
(use_declaration 
  argument: (scoped_identifier 
    name: (identifier) @def.import))

; Use declarations - scoped use path chain
(scoped_use_list
  path: (identifier) @def.import)

(scoped_use_list
  path: (scoped_identifier
    name: (identifier) @def.import))

; ========== USAGES ==========

; Function calls
(call_expression 
  function: (identifier) @usage.call)

; Method calls: obj.method()
(call_expression 
  function: (field_expression 
    field: (field_identifier) @usage.method))

; Scoped calls: std::println!()
(call_expression 
  function: (scoped_identifier 
    name: (identifier) @usage.call))

; Field access
(field_expression 
  field: (field_identifier) @usage.field)

; Type usage
(type_identifier) @usage.type

; Generic identifier usage (CATCH-ALL)
(identifier) @usage.variable
(function_signature_item name: (identifier) @def.function)
(type_item name: (type_identifier) @def.type)
(macro_definition name: (identifier) @def.function)
