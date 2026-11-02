(comment) @comment

(section_keyword) @keyword

(header_entity name: (identifier) @function)

(entity_instance name: (identifier) @type)
(complex_instance name: (identifier) @type)
(typed_value type: (identifier) @type.builtin)

(instance_id) @label
(reference) @variable

(string) @string
(enum_value) @constant
(null) @constant.builtin
(derived) @constant.builtin
(real) @number
(integer) @number
(binary) @number

"=" @operator
["(" ")"] @punctuation.bracket
["," ";"] @punctuation.delimiter
