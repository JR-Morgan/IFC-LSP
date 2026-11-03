/**
 * Tree-sitter grammar for EXPRESS (ISO 10303-11) schema files.
 * Deliberately shallow: declarations are recognised by their header, and
 * everything else is tokenised for highlighting only.
 */

/// <reference types="tree-sitter-cli/dsl" />
// @ts-check

const KEYWORDS = [
  "ABSTRACT", "AGGREGATE", "ALIAS", "AND", "ANDOR", "ARRAY", "AS", "BAG", "BASED_ON", "BEGIN",
  "BY", "CASE", "CONSTANT", "DERIVE", "ELSE", "END", "END_ALIAS", "END_CASE", "END_CONSTANT",
  "END_ENTITY", "END_FUNCTION", "END_IF", "END_LOCAL", "END_PROCEDURE", "END_REPEAT",
  "END_RULE", "END_SCHEMA", "END_SUBTYPE_CONSTRAINT", "END_TYPE", "ENTITY", "ENUMERATION",
  "ESCAPE", "EXTENSIBLE", "FIXED", "FOR", "FROM", "FUNCTION", "GENERIC", "GENERIC_ENTITY",
  "IF", "IN", "INVERSE", "LIKE", "LIST", "LOCAL", "MOD", "NOT", "OF", "ONEOF", "OPTIONAL",
  "OR", "OTHERWISE", "PROCEDURE", "QUERY", "REFERENCE", "RENAMED", "REPEAT", "RETURN",
  "RULE", "SCHEMA", "SELECT", "SELF", "SET", "SKIP", "SUBTYPE", "SUBTYPE_CONSTRAINT",
  "SUPERTYPE", "THEN", "TO", "TOTAL_OVER", "TYPE", "UNIQUE", "UNTIL", "USE", "VAR", "WHERE",
  "WHILE", "WITH", "XOR",
];

const BUILTIN_TYPES = ["REAL", "INTEGER", "NUMBER", "BOOLEAN", "LOGICAL", "STRING", "BINARY"];

const caseInsensitive = (word) =>
  new RegExp(
    word
      .split("")
      .map((c) => (/[a-z]/i.test(c) ? `[${c.toUpperCase()}${c.toLowerCase()}]` : c))
      .join(""),
  );

module.exports = grammar({
  name: "express",

  extras: ($) => [/\s/, $.comment, $.line_comment],

  word: ($) => $.identifier,

  rules: {
    source_file: ($) => repeat(choice($.declaration, $._token)),

    declaration: ($) =>
      seq(field("keyword", $.declaration_keyword), field("name", $.identifier)),

    declaration_keyword: (_) =>
      token(
        prec(
          2,
          choice(
            ...["SCHEMA", "ENTITY", "TYPE", "FUNCTION", "RULE", "PROCEDURE", "SUBTYPE_CONSTRAINT"].map(
              caseInsensitive,
            ),
          ),
        ),
      ),

    _token: ($) =>
      choice(
        $.keyword,
        $.builtin_type,
        $.constant,
        $.identifier,
        $.number,
        $.string,
        $.operator,
        $.punctuation,
        // Brackets stay anonymous tokens so editor bracket queries can match them.
        "(",
        ")",
        "[",
        "]",
        "{",
        "}",
      ),

    keyword: (_) => token(prec(1, choice(...KEYWORDS.map(caseInsensitive)))),

    builtin_type: (_) => token(prec(1, choice(...BUILTIN_TYPES.map(caseInsensitive)))),

    constant: (_) =>
      token(prec(1, choice(...["TRUE", "FALSE", "UNKNOWN", "PI", "CONST_E"].map(caseInsensitive), "?"))),

    identifier: (_) => /[A-Za-z_][A-Za-z0-9_]*/,

    number: (_) => /[0-9]+(\.[0-9]*)?([Ee][-+]?[0-9]+)?/,

    string: (_) => token(seq("'", repeat(choice(/[^']/, "''")), "'")),

    operator: (_) =>
      token(choice(":=:", ":<>:", ":=", "<*", "||", "<=", ">=", "<>", "**", "+", "-", "*", "/", "=", "<", ">", "|", "\\", ".")),

    punctuation: (_) => token(choice(";", ":", ",")),

    comment: (_) => token(seq("(*", /[^*]*\*+([^)*][^*]*\*+)*/, ")")),

    line_comment: (_) => token(seq("--", /.*/)),
  },
});
