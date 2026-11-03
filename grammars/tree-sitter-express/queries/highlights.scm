(comment) @comment
(line_comment) @comment

(declaration keyword: (declaration_keyword) @keyword)
(declaration name: (identifier) @type)

(keyword) @keyword
(builtin_type) @type.builtin
(constant) @constant.builtin
(number) @number
(string) @string
(operator) @operator
(punctuation) @punctuation.delimiter
["(" ")" "[" "]" "{" "}"] @punctuation.bracket
