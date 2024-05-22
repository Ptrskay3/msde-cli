use winnow::ascii::space0;
use winnow::combinator::{alt, delimited, preceded, terminated};
use winnow::error::StrContext;
use winnow::prelude::PResult;
use winnow::token::{literal, take_while};
use winnow::Parser;

#[derive(Debug, PartialEq)]
pub enum OkVariant<'a> {
    Uuid(uuid::Uuid),
    String(&'a str),
}

#[derive(Debug, PartialEq)]
pub enum ElixirTuple<'a> {
    ErrorEx(&'a str),
    OkEx(OkVariant<'a>),
}

fn is_atom_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn is_uuid_char(c: char) -> bool {
    c.is_ascii_hexdigit() || c == '-'
}

fn parse_atom<'a>(input: &'_ mut &'a str) -> PResult<&'a str> {
    delimited(
        space0,
        delimited(literal(":"), take_while(1.., is_atom_char), space0),
        space0,
    )
    .context(StrContext::Label("atom"))
    .parse_next(input)
}

fn parse_uuid<'a>(input: &mut &'a str) -> PResult<uuid::Uuid> {
    delimited(
        space0,
        preceded(
            literal("\""),
            terminated(take_while(1.., is_uuid_char), literal("\"")),
        ),
        space0,
    )
    .context(StrContext::Label("uuid"))
    .parse_to()
    .parse_next(input)
}

fn parse_quoted_string<'a>(input: &'_ mut &'a str) -> PResult<&'a str> {
    delimited(
        space0,
        preceded(
            literal("\""),
            terminated(take_while(1.., |c| c != '\"'), literal("\"")),
        ),
        space0,
    )
    .context(StrContext::Label("quoted_string"))
    .parse_next(input)
}

fn parse_ok_variant<'a>(input: &mut &'a str) -> PResult<OkVariant<'a>> {
    alt((
        parse_uuid.map(OkVariant::Uuid),
        parse_quoted_string.map(OkVariant::String),
    ))
    .context(StrContext::Label("ok_variant"))
    .parse_next(input)
}

fn parse_error<'a>(input: &mut &'a str) -> PResult<ElixirTuple<'a>> {
    (
        literal("{"),
        delimited(space0, literal(":error"), space0),
        delimited(space0, literal(","), space0),
        parse_atom,
        delimited(space0, literal("}"), space0),
    )
        .map(|(_, _, _, atom, _)| ElixirTuple::ErrorEx(atom))
        .context(StrContext::Label("error_tuple"))
        .parse_next(input)
}

fn parse_ok<'a>(input: &mut &'a str) -> PResult<ElixirTuple<'a>> {
    (
        literal("{"),
        delimited(space0, literal(":ok"), space0),
        delimited(space0, literal(","), space0),
        parse_ok_variant,
        delimited(space0, literal("}"), space0),
    )
        .context(StrContext::Label("ok_tuple"))
        .map(|(_, _, _, variant, _)| ElixirTuple::OkEx(variant))
        .parse_next(input)
}

/// Parses two-element simple Elixir ok and error tuples _reliably_. These usually come from the Game.start and Game.sync calls.
pub fn parse_simple_tuple<'a>(input: &mut &'a str) -> PResult<ElixirTuple<'a>> {
    terminated(alt((parse_error, parse_ok)), space0).parse_next(input)
}

// TODO: Add unit tests
