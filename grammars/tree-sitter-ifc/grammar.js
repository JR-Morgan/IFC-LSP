/**
 * Tree-sitter grammar for ISO 10303-21 (STEP Part 21) exchange files, as used by IFC.
 * Structural only: argument values are not checked against any schema.
 */

/// <reference types="tree-sitter-cli/dsl" />
// @ts-check

const commaSep = (rule) => optional(seq(rule, repeat(seq(",", rule))));

module.exports = grammar({
  name: "ifc",

  extras: ($) => [/\s/, $.comment],

  word: ($) => $.identifier,

  rules: {
    source_file: ($) => repeat($._statement),

    _statement: ($) =>
      choice($.section_statement, $.header_entity, $.entity_instance),

    section_statement: ($) => seq($.section_keyword, ";"),

    section_keyword: (_) =>
      token(
        choice(
          "ISO-10303-21",
          "END-ISO-10303-21",
          "HEADER",
          "ENDSEC",
          "DATA",
        ),
      ),

    header_entity: ($) =>
      seq(field("name", $.identifier), field("arguments", $.argument_list), ";"),

    entity_instance: ($) =>
      seq(
        field("id", $.instance_id),
        "=",
        choice(
          seq(field("name", $.identifier), field("arguments", $.argument_list)),
          $.complex_instance,
        ),
        ";",
      ),

    complex_instance: ($) =>
      seq(
        "(",
        repeat1(seq(field("name", $.identifier), field("arguments", $.argument_list))),
        ")",
      ),

    argument_list: ($) => seq("(", commaSep($._value), ")"),

    _value: ($) =>
      choice(
        $.null,
        $.derived,
        $.reference,
        $.string,
        $.enum_value,
        $.real,
        $.integer,
        $.binary,
        $.typed_value,
        $.list,
      ),

    list: ($) => seq("(", commaSep($._value), ")"),

    typed_value: ($) => seq(field("type", $.identifier), "(", $._value, ")"),

    null: (_) => "$",
    derived: (_) => "*",

    instance_id: (_) => /#[0-9]+/,
    reference: (_) => /#[0-9]+/,

    identifier: (_) => /[A-Za-z_][A-Za-z0-9_]*/,

    string: (_) => token(seq("'", repeat(choice(/[^']/, "''")), "'")),

    enum_value: (_) => /\.[A-Za-z_][A-Za-z0-9_]*\./,

    real: (_) =>
      token(
        choice(
          /[-+]?[0-9]+\.[0-9]*([Ee][-+]?[0-9]+)?/,
          /[-+]?[0-9]+[Ee][-+]?[0-9]+/,
          /[-+]?\.[0-9]+([Ee][-+]?[0-9]+)?/,
        ),
      ),

    integer: (_) => /[-+]?[0-9]+/,

    binary: (_) => /"[0-9A-Fa-f]*"/,

    comment: (_) => token(seq("/*", /[^*]*\*+([^/*][^*]*\*+)*/, "/")),
  },
});
