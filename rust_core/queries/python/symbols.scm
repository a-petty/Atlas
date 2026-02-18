; === PYTHON SYMBOL QUERIES (v5 - Specific Usages) ===

; ========== DEFINITIONS ==========

(function_definition name: (identifier) @def.function)
(class_definition name: (identifier) @def.class)

; Handle assignments (e.g., `x = 10`)
(assignment left: (identifier) @def.variable)

; from ... import d as e -> `e` is the definition (the alias)
(aliased_import alias: (identifier) @def.alias)

; Comprehensions (e.g., `[x for x in iterable]`)
(list_comprehension (for_in_clause left: (identifier) @def.variable))
(dictionary_comprehension (for_in_clause left: (identifier) @def.variable))
(set_comprehension (for_in_clause left: (identifier) @def.variable))
(generator_expression (for_in_clause left: (identifier) @def.variable))


; ========== USAGES ==========
; This is now a "whitelist" of common usage patterns, which avoids
; accidentally capturing identifiers from import paths.

; Function calls (e.g., `my_func()`)
(call function: (identifier) @usage.call)
; Method calls (e.g., `obj.my_method()`)
(call function: (attribute attribute: (identifier) @usage.method))

; Using an identifier as an argument (e.g., `my_func(my_var)`)
(call arguments: (argument_list (identifier) @usage.argument))

; Using an identifier as a keyword argument value (e.g., `my_func(kw=my_var)`)
(keyword_argument value: (identifier) @usage.argument)

; Using an identifier in an attribute access (e.g., `my_obj.some_attr`)
(attribute object: (identifier) @usage.variable)

; Using an identifier on the right side of an assignment (e.g., `x = my_var`)
(assignment right: (identifier) @usage.variable)
(assignment right: (attribute object: (identifier) @usage.variable))

; Using an identifier as a type annotation
(type (identifier) @usage.type)

; Using an identifier in a return statement
(return_statement (identifier) @usage.variable)
(return_statement (attribute object: (identifier) @usage.variable))

; An identifier being subscribed (e.g. `my_dict[key]`)
(subscript value: (identifier) @usage.variable)
