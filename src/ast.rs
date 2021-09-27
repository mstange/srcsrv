use crate::errors::{EvalError, ParseError};
use std::result::Result;

use memchr::{memchr, memchr2};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AstNode<'a> {
    /// String concatenation of the evaluated child nodes.
    Sequence(Vec<AstNode<'a>>),
    /// A literal string.
    LiteralString(&'a str),
    /// Substitute with the value of the variable with this name.
    Variable(&'a str),
    /// Substitute with the value of the variable whose name is given by the
    /// value of the variable with this name.
    FnVar(Box<AstNode<'a>>),
    /// Substitute with the string but with all slashes replaced by backslashes.
    FnBackslash(Box<AstNode<'a>>),
    /// Substitute with the file name extracted from the path.
    FnFile(Box<AstNode<'a>>),
}

impl<'a> AstNode<'a> {
    pub fn try_from_str(s: &'a str) -> Result<AstNode<'a>, ParseError> {
        if s.is_empty() {
            return Ok(AstNode::LiteralString(""));
        }
        let s = s.as_bytes();
        let (node, _rest) = Self::try_parse_all(s, false)?;
        Ok(node)
    }

    fn try_parse_all(s: &'a [u8], nested: bool) -> Result<(AstNode<'a>, &'a [u8]), ParseError> {
        let (node, rest) = Self::try_parse(s, false)?;
        if rest.is_empty() || (nested && rest[0] == b')') {
            return Ok((node, rest));
        }

        let mut nodes = vec![node];
        let mut rest = rest;
        loop {
            let (node, r) = Self::try_parse(rest, false)?;
            nodes.push(node);
            rest = r;
            if rest.is_empty() || (nested && rest[0] == b')') {
                return Ok((AstNode::Sequence(nodes), rest));
            }
        }
    }

    // s must not be empty
    fn try_parse(s: &'a [u8], nested: bool) -> Result<(AstNode<'a>, &'a [u8]), ParseError> {
        if s[0] != b'%' {
            // We have a literal at the beginning.
            let literal_end = if nested {
                memchr2(b'%', b')', s)
            } else {
                memchr(b'%', s)
            };
            let literal_end = literal_end.unwrap_or(s.len());
            let (literal, rest) = s.split_at(literal_end);
            let string = std::str::from_utf8(literal).map_err(|_| ParseError::InvalidUtf8)?;
            return Ok((AstNode::LiteralString(string), rest));
        }

        // We start with a %.
        let s = &s[1..];
        let second_percent_pos = memchr(b'%', s).ok_or(ParseError::MissingPercent)?;
        let rest = &s[second_percent_pos + 1..];
        let var_name =
            std::str::from_utf8(&s[..second_percent_pos]).map_err(|_| ParseError::InvalidUtf8)?;
        match var_name.to_ascii_lowercase().as_str() {
            "fnvar" => {
                let (node, rest) = Self::try_parse_args(rest)?;
                Ok((AstNode::FnVar(Box::new(node)), rest))
            }
            "fnbksl" => {
                let (node, rest) = Self::try_parse_args(rest)?;
                Ok((AstNode::FnBackslash(Box::new(node)), rest))
            }
            "fnfile" => {
                let (node, rest) = Self::try_parse_args(rest)?;
                Ok((AstNode::FnFile(Box::new(node)), rest))
            }
            _ => Ok((AstNode::Variable(var_name), rest)),
        }
    }

    fn try_parse_args(s: &'a [u8]) -> Result<(AstNode<'a>, &'a [u8]), ParseError> {
        if s.is_empty() || s[0] != b'(' {
            return Err(ParseError::MissingOpeningBracket);
        }
        let (node, rest) = Self::try_parse_all(&s[1..], true)?;
        if rest.is_empty() || rest[0] != b')' {
            return Err(ParseError::MissingClosingBracket);
        }
        Ok((node, &rest[1..]))
    }

    pub fn eval<F>(&self, f: &mut F) -> Result<String, EvalError>
    where
        F: FnMut(&str) -> Result<String, EvalError>,
    {
        match self {
            AstNode::Sequence(nodes) => {
                let values: Result<Vec<String>, EvalError> =
                    nodes.iter().map(|node| node.eval(f)).collect();
                Ok(values?.join(""))
            }
            AstNode::LiteralString(s) => Ok(s.to_string()),
            AstNode::Variable(var_name) => f(var_name),
            AstNode::FnVar(node) => {
                let var_name = node.eval(f)?;
                f(&var_name)
            }
            AstNode::FnBackslash(node) => {
                let val = node.eval(f)?;
                Ok(val.replace('/', "\\"))
            }
            AstNode::FnFile(node) => {
                let val = node.eval(f)?;
                match val.rsplit_once('\\') {
                    Some((_base, file)) => Ok(file.to_string()),
                    None => Ok(val),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{AstNode, ParseError};

    #[test]
    fn it_works() -> Result<(), ParseError> {
        assert_eq!(
            AstNode::try_from_str("hello")?,
            AstNode::LiteralString("hello")
        );
        assert_eq!(
            AstNode::try_from_str("hello%world%")?,
            AstNode::Sequence(vec![
                AstNode::LiteralString("hello"),
                AstNode::Variable("world")
            ])
        );
        assert_eq!(
            AstNode::try_from_str("%hello%world")?,
            AstNode::Sequence(vec![
                AstNode::Variable("hello"),
                AstNode::LiteralString("world")
            ])
        );
        Ok(())
    }
}
