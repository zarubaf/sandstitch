// SPDX-License-Identifier: Apache-2.0

use num_bigint::{BigInt, BigUint, Sign};
use num_traits::Zero;
use slang_sys::{Field, Range, Type, Variant};

use pest::Parser;
use pest_derive::Parser;
use std::error::Error;

#[derive(Parser)]
#[grammar = "extract/grammar.pest"]
struct DataTypeParser;

/// Parses a slang type definition (from --ast-json) into a `Type`
pub fn parse_type_definition(input: &str) -> Result<Type, Box<dyn Error>> {
    let mut parse_tree = DataTypeParser::parse(Rule::top, input)?;
    let ty = parse_tree.next().unwrap().into_inner().next().unwrap();
    Ok(build_field_type(ty))
}

fn build_field_type(pair: pest::iterators::Pair<Rule>) -> Type {
    let inner_pair = pair.into_inner().next().unwrap();
    match inner_pair.as_rule() {
        Rule::logic_type => build_logic_type(inner_pair, None, false),
        Rule::struct_type => build_struct_or_union_type(inner_pair, false),
        Rule::union_type => build_struct_or_union_type(inner_pair, true),
        Rule::enum_type => build_enum_type(inner_pair),
        Rule::int_type => build_logic_type(inner_pair, Some(Range { msb: 31, lsb: 0 }), true),
        Rule::longint_type => build_logic_type(inner_pair, Some(Range { msb: 63, lsb: 0 }), true),
        Rule::integer_type => build_logic_type(inner_pair, Some(Range { msb: 31, lsb: 0 }), true),
        Rule::shortint_type => build_logic_type(inner_pair, Some(Range { msb: 15, lsb: 0 }), true),
        Rule::byte_type => build_logic_type(inner_pair, Some(Range { msb: 7, lsb: 0 }), true),
        Rule::time_type => build_logic_type(inner_pair, Some(Range { msb: 63, lsb: 0 }), false),
        _ => unreachable!(),
    }
}

fn build_logic_type(
    pair: pest::iterators::Pair<Rule>,
    extra_packed_dimension: Option<Range>,
    default_signed: bool,
) -> Type {
    let inner = pair.into_inner();

    let mut signed = default_signed;
    let mut packed_dimensions = Vec::new();
    let mut unpacked_dimensions = Vec::new();

    for inner_pair in inner {
        match inner_pair.as_rule() {
            Rule::dimensions => {
                let inner_inner = inner_pair.into_inner();
                for inner_inner_pair in inner_inner {
                    match inner_inner_pair.as_rule() {
                        Rule::packed_dimensions => {
                            for dim_pair in inner_inner_pair.into_inner() {
                                let range = build_range(dim_pair.into_inner().next().unwrap());
                                packed_dimensions.push(range);
                            }
                        }
                        Rule::unpacked_dimensions => {
                            for dim_pair in inner_inner_pair.into_inner() {
                                let range = build_range(dim_pair.into_inner().next().unwrap());
                                unpacked_dimensions.push(range);
                            }
                        }
                        _ => {}
                    }
                }
            }
            Rule::signed_modifier => match inner_pair.into_inner().next().unwrap().as_rule() {
                Rule::signed_keyword => signed = true,
                Rule::unsigned_keyword => signed = false,
                _ => unreachable!(),
            },
            _ => {}
        }
    }

    if let Some(final_packed_dimension) = extra_packed_dimension {
        packed_dimensions.push(final_packed_dimension);
    }

    Type::Logic {
        signed,
        packed_dimensions,
        unpacked_dimensions,
    }
}

fn build_struct_or_union_type(pair: pest::iterators::Pair<Rule>, is_union: bool) -> Type {
    let inner = pair.into_inner();
    let mut fields = Vec::new();
    let mut packed_dimensions = Vec::new();
    let mut unpacked_dimensions = Vec::new();
    let mut name = String::new();
    let mut signed = false;
    let mut packed = false;

    for inner_pair in inner {
        match inner_pair.as_rule() {
            Rule::packed_keyword => packed = true,
            Rule::signed_modifier => {
                signed = inner_pair.into_inner().next().unwrap().as_rule() == Rule::signed_keyword
            }
            Rule::field_list => {
                for field_pair in inner_pair.into_inner() {
                    if field_pair.as_rule() == Rule::field {
                        let field = build_field(field_pair);
                        fields.push(field);
                    }
                }
            }
            Rule::full_identifier => {
                name = inner_pair.as_str().to_string();
            }
            Rule::dimensions => {
                let inner_inner = inner_pair.into_inner();
                for inner_inner_pair in inner_inner {
                    match inner_inner_pair.as_rule() {
                        Rule::packed_dimensions => {
                            for dim_pair in inner_inner_pair.into_inner() {
                                let range = build_range(dim_pair.into_inner().next().unwrap());
                                packed_dimensions.push(range);
                            }
                        }
                        Rule::unpacked_dimensions => {
                            for dim_pair in inner_inner_pair.into_inner() {
                                let range = build_range(dim_pair.into_inner().next().unwrap());
                                unpacked_dimensions.push(range);
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    if is_union {
        Type::Union {
            name,
            signed,
            packed,
            fields,
            packed_dimensions,
            unpacked_dimensions,
        }
    } else {
        Type::Struct {
            name,
            signed,
            packed,
            fields,
            packed_dimensions,
            unpacked_dimensions,
        }
    }
}

fn build_enum_type(pair: pest::iterators::Pair<Rule>) -> Type {
    let inner = pair.into_inner();

    let mut variants = Vec::new();
    let mut packed_dimensions = Vec::new();
    let mut unpacked_dimensions = Vec::new();
    let mut name = String::new();

    for inner_pair in inner {
        match inner_pair.as_rule() {
            Rule::variant_list => {
                for variant_pair in inner_pair.into_inner() {
                    if variant_pair.as_rule() == Rule::variant {
                        let variant = build_variant(variant_pair);
                        variants.push(variant);
                    }
                }
            }
            Rule::full_identifier => {
                name = inner_pair.as_str().to_string();
            }
            Rule::dimensions => {
                let inner_inner = inner_pair.into_inner();
                for inner_inner_pair in inner_inner {
                    match inner_inner_pair.as_rule() {
                        Rule::packed_dimensions => {
                            for dim_pair in inner_inner_pair.into_inner() {
                                let range = build_range(dim_pair.into_inner().next().unwrap());
                                packed_dimensions.push(range);
                            }
                        }
                        Rule::unpacked_dimensions => {
                            for dim_pair in inner_inner_pair.into_inner() {
                                let range = build_range(dim_pair.into_inner().next().unwrap());
                                unpacked_dimensions.push(range);
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    Type::Enum {
        name,
        variants,
        packed_dimensions,
        unpacked_dimensions,
    }
}

fn build_field(pair: pest::iterators::Pair<Rule>) -> Field {
    let mut inner = pair.into_inner();
    let field_type_pair = inner.next().unwrap();
    let ty = build_field_type(field_type_pair);
    let name = inner.next().unwrap().as_str();
    Field {
        name: String::from(name).clone(),
        ty,
    }
}

fn build_variant(pair: pest::iterators::Pair<Rule>) -> Variant {
    let mut inner = pair.into_inner();

    let name = inner.next().unwrap().as_str().to_string();

    let mut negative = false;
    let mut signed = false;
    let mut numbers = Vec::new();
    for part in inner.next().unwrap().into_inner() {
        match part.as_rule() {
            Rule::negative_sign => negative = true,
            Rule::signed_literal => signed = true,
            Rule::non_negative_integer => numbers.push(part.as_str()),
            _ => unreachable!(),
        }
    }
    let [width, magnitude_str] = numbers[..] else {
        unreachable!("a sized decimal has a width and a magnitude")
    };

    let width = width.parse::<usize>().unwrap();
    let magnitude = BigUint::parse_bytes(magnitude_str.as_bytes(), 10).unwrap();

    let sign = if magnitude.is_zero() {
        Sign::NoSign
    } else if negative {
        Sign::Minus
    } else {
        Sign::Plus
    };

    Variant {
        name,
        width,
        value: BigInt::from_biguint(sign, magnitude),
        signed,
    }
}

fn build_range(pair: pest::iterators::Pair<Rule>) -> Range {
    let mut inner = pair.into_inner();
    let msb = inner.next().unwrap().as_str().parse::<i32>().unwrap();
    let lsb = inner.next().unwrap().as_str().parse::<i32>().unwrap();
    Range { msb, lsb }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_enum() {
        let type_def = parse_type_definition("enum{a=42'd1,b=-42'sd2,c=42'd3}my_enum").unwrap();
        assert_eq!(
            type_def,
            Type::Enum {
                name: "my_enum".to_string(),
                variants: vec![
                    Variant {
                        name: "a".to_string(),
                        width: 42,
                        value: BigInt::from(1),
                        signed: false,
                    },
                    Variant {
                        name: "b".to_string(),
                        width: 42,
                        value: BigInt::from(-2),
                        signed: true,
                    },
                    Variant {
                        name: "c".to_string(),
                        width: 42,
                        value: BigInt::from(3),
                        signed: false,
                    },
                ],
                packed_dimensions: vec![],
                unpacked_dimensions: vec![],
            }
        );
    }

    #[test]
    fn test_int() {
        let type_def = parse_type_definition("int").unwrap();
        assert_eq!(
            type_def,
            Type::Logic {
                signed: true,
                packed_dimensions: vec![Range { msb: 31, lsb: 0 }],
                unpacked_dimensions: vec![],
            }
        );
    }

    #[test]
    fn test_int_unsigned() {
        let type_def = parse_type_definition("int unsigned").unwrap();
        assert_eq!(
            type_def,
            Type::Logic {
                signed: false,
                packed_dimensions: vec![Range { msb: 31, lsb: 0 }],
                unpacked_dimensions: vec![],
            }
        );
    }

    #[test]
    fn test_int_long() {
        let type_def = parse_type_definition("longint").unwrap();
        assert_eq!(
            type_def,
            Type::Logic {
                signed: true,
                packed_dimensions: vec![Range { msb: 63, lsb: 0 }],
                unpacked_dimensions: vec![],
            }
        );
    }

    #[test]
    fn test_int_long_unsigned() {
        let type_def = parse_type_definition("longint unsigned").unwrap();
        assert_eq!(
            type_def,
            Type::Logic {
                signed: false,
                packed_dimensions: vec![Range { msb: 63, lsb: 0 }],
                unpacked_dimensions: vec![],
            }
        );
    }
}
