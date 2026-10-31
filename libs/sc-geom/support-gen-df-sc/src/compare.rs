//! Whether two values serialize to the same data once one run's ids are
//! named as the other's (#304), without writing either out as text.
//!
//! Every run names its supports under an id prefix of its own, so two runs
//! of the same settings on the same models differ only there. The kept value
//! is walked once into a list of tokens; the fresh one is then walked against
//! that list, stopping at the first difference. A fresh string matches when
//! it reads as the kept one with each occurrence of the fresh prefix replaced
//! by the kept prefix. Numbers compare by their bits, every value that is not
//! finite as one (a JSON document writes them all as `null`), so the verdict
//! is the one comparing the two documents' JSON would give.

use std::fmt;

use serde::Serialize;
use serde::ser::{self, Serializer};

/// Whether `fresh` serializes as `kept` does once each `fresh_prefix` in its
/// strings reads `kept_prefix`.
pub(crate) fn same_except_prefix<T: Serialize + ?Sized>(
    kept: &T,
    kept_prefix: &str,
    fresh: &T,
    fresh_prefix: &str,
) -> bool {
    let mut record = Walk::Record {
        tokens: Vec::new(),
        text: String::new(),
    };
    if kept.serialize(&mut record).is_err() {
        return false;
    }
    let Walk::Record { tokens, text } = record else {
        unreachable!("a record stays a record")
    };
    let mut compare = Walk::Compare {
        tokens: &tokens,
        text: &text,
        at: 0,
        kept_prefix,
        fresh_prefix,
    };
    if fresh.serialize(&mut compare).is_err() {
        return false;
    }
    matches!(compare, Walk::Compare { at, .. } if at == tokens.len())
}

/// One step of a serialized value, kept to 16 bytes: strings point into the
/// record's text, and a 128-bit number is its high half then its low half.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Token {
    Bool(bool),
    I64(i64),
    U64(u64),
    I128High(i64),
    U128High(u64),
    F32(u32),
    F64(u64),
    /// No value: `None`, a unit, or a number that is not finite. As in
    /// JSON, `Some(value)` is just the value.
    Null,
    Str(u32, u32),
    /// A struct field left out (`skip_serializing_if`); the fields written
    /// are told apart by their place, as the two values share a type.
    Skipped,
    Variant(u32),
    /// The start of a sequence, tuple, map or struct.
    Open,
    Close,
}

/// A walk over a value: recording it, or comparing it with a record.
enum Walk<'a> {
    Record {
        tokens: Vec<Token>,
        text: String,
    },
    Compare {
        tokens: &'a [Token],
        text: &'a str,
        at: usize,
        kept_prefix: &'a str,
        fresh_prefix: &'a str,
    },
}

/// The values differ (or one could not be walked).
#[derive(Debug)]
pub(crate) struct Differ;

impl fmt::Display for Differ {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the values differ")
    }
}

impl std::error::Error for Differ {}

impl ser::Error for Differ {
    fn custom<M: fmt::Display>(_: M) -> Self {
        Differ
    }
}

impl Walk<'_> {
    fn token(&mut self, token: Token) -> Result<(), Differ> {
        match self {
            Walk::Record { tokens, .. } => {
                tokens.push(token);
                Ok(())
            }
            Walk::Compare { tokens, at, .. } => {
                if tokens.get(*at) != Some(&token) {
                    return Err(Differ);
                }
                *at += 1;
                Ok(())
            }
        }
    }

    fn string(&mut self, s: &str) -> Result<(), Differ> {
        match self {
            Walk::Record { tokens, text } => {
                let start = u32::try_from(text.len()).map_err(|_| Differ)?;
                text.push_str(s);
                let end = u32::try_from(text.len()).map_err(|_| Differ)?;
                tokens.push(Token::Str(start, end));
                Ok(())
            }
            Walk::Compare {
                tokens,
                text,
                at,
                kept_prefix,
                fresh_prefix,
            } => {
                let Some(&Token::Str(start, end)) = tokens.get(*at) else {
                    return Err(Differ);
                };
                let kept = &text[start as usize..end as usize];
                if !reads_as(kept, s, kept_prefix, fresh_prefix) {
                    return Err(Differ);
                }
                *at += 1;
                Ok(())
            }
        }
    }

    fn float(&mut self, finite: bool, token: Token) -> Result<(), Differ> {
        self.token(if finite { token } else { Token::Null })
    }
}

/// Whether `fresh` with every `fresh_prefix` replaced by `kept_prefix`
/// (left to right, as `str::replace` does) is `kept`.
fn reads_as(kept: &str, fresh: &str, kept_prefix: &str, fresh_prefix: &str) -> bool {
    if fresh_prefix.is_empty() {
        return kept == fresh;
    }
    let mut rest = kept;
    for (k, piece) in fresh.split(fresh_prefix).enumerate() {
        if k > 0 {
            let Some(after) = rest.strip_prefix(kept_prefix) else {
                return false;
            };
            rest = after;
        }
        let Some(after) = rest.strip_prefix(piece) else {
            return false;
        };
        rest = after;
    }
    rest.is_empty()
}

impl<'w, 'a> Serializer for &'w mut Walk<'a> {
    type Ok = ();
    type Error = Differ;
    type SerializeSeq = Self;
    type SerializeTuple = Self;
    type SerializeTupleStruct = Self;
    type SerializeTupleVariant = Self;
    type SerializeMap = Self;
    type SerializeStruct = Self;
    type SerializeStructVariant = Self;

    fn serialize_bool(self, v: bool) -> Result<(), Differ> {
        self.token(Token::Bool(v))
    }
    fn serialize_i8(self, v: i8) -> Result<(), Differ> {
        self.token(Token::I64(v.into()))
    }
    fn serialize_i16(self, v: i16) -> Result<(), Differ> {
        self.token(Token::I64(v.into()))
    }
    fn serialize_i32(self, v: i32) -> Result<(), Differ> {
        self.token(Token::I64(v.into()))
    }
    fn serialize_i64(self, v: i64) -> Result<(), Differ> {
        self.token(Token::I64(v))
    }
    fn serialize_i128(self, v: i128) -> Result<(), Differ> {
        self.token(Token::I128High((v >> 64) as i64))?;
        self.token(Token::U64(v as u64))
    }
    fn serialize_u8(self, v: u8) -> Result<(), Differ> {
        self.token(Token::U64(v.into()))
    }
    fn serialize_u16(self, v: u16) -> Result<(), Differ> {
        self.token(Token::U64(v.into()))
    }
    fn serialize_u32(self, v: u32) -> Result<(), Differ> {
        self.token(Token::U64(v.into()))
    }
    fn serialize_u64(self, v: u64) -> Result<(), Differ> {
        self.token(Token::U64(v))
    }
    fn serialize_u128(self, v: u128) -> Result<(), Differ> {
        self.token(Token::U128High((v >> 64) as u64))?;
        self.token(Token::U64(v as u64))
    }
    fn serialize_f32(self, v: f32) -> Result<(), Differ> {
        self.float(v.is_finite(), Token::F32(v.to_bits()))
    }
    fn serialize_f64(self, v: f64) -> Result<(), Differ> {
        self.float(v.is_finite(), Token::F64(v.to_bits()))
    }
    fn serialize_char(self, v: char) -> Result<(), Differ> {
        self.string(v.encode_utf8(&mut [0; 4]))
    }
    fn serialize_str(self, v: &str) -> Result<(), Differ> {
        self.string(v)
    }
    fn serialize_bytes(self, v: &[u8]) -> Result<(), Differ> {
        self.token(Token::Open)?;
        for b in v {
            self.token(Token::U64((*b).into()))?;
        }
        self.token(Token::Close)
    }
    fn serialize_none(self) -> Result<(), Differ> {
        self.token(Token::Null)
    }
    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<(), Differ> {
        value.serialize(self)
    }
    fn serialize_unit(self) -> Result<(), Differ> {
        self.token(Token::Null)
    }
    fn serialize_unit_struct(self, _: &'static str) -> Result<(), Differ> {
        self.token(Token::Null)
    }
    fn serialize_unit_variant(
        self,
        _: &'static str,
        index: u32,
        _: &'static str,
    ) -> Result<(), Differ> {
        self.token(Token::Variant(index))
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        value: &T,
    ) -> Result<(), Differ> {
        value.serialize(self)
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        index: u32,
        _: &'static str,
        value: &T,
    ) -> Result<(), Differ> {
        self.token(Token::Variant(index))?;
        value.serialize(self)
    }
    fn serialize_seq(self, _: Option<usize>) -> Result<Self, Differ> {
        self.token(Token::Open)?;
        Ok(self)
    }
    fn serialize_tuple(self, _: usize) -> Result<Self, Differ> {
        self.token(Token::Open)?;
        Ok(self)
    }
    fn serialize_tuple_struct(self, _: &'static str, _: usize) -> Result<Self, Differ> {
        self.token(Token::Open)?;
        Ok(self)
    }
    fn serialize_tuple_variant(
        self,
        _: &'static str,
        index: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Self, Differ> {
        self.token(Token::Variant(index))?;
        self.token(Token::Open)?;
        Ok(self)
    }
    fn serialize_map(self, _: Option<usize>) -> Result<Self, Differ> {
        self.token(Token::Open)?;
        Ok(self)
    }
    fn serialize_struct(self, _: &'static str, _: usize) -> Result<Self, Differ> {
        self.token(Token::Open)?;
        Ok(self)
    }
    fn serialize_struct_variant(
        self,
        _: &'static str,
        index: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Self, Differ> {
        self.token(Token::Variant(index))?;
        self.token(Token::Open)?;
        Ok(self)
    }
}

impl ser::SerializeSeq for &mut Walk<'_> {
    type Ok = ();
    type Error = Differ;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Differ> {
        value.serialize(&mut **self)
    }
    fn end(self) -> Result<(), Differ> {
        self.token(Token::Close)
    }
}

impl ser::SerializeTuple for &mut Walk<'_> {
    type Ok = ();
    type Error = Differ;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Differ> {
        value.serialize(&mut **self)
    }
    fn end(self) -> Result<(), Differ> {
        self.token(Token::Close)
    }
}

impl ser::SerializeTupleStruct for &mut Walk<'_> {
    type Ok = ();
    type Error = Differ;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Differ> {
        value.serialize(&mut **self)
    }
    fn end(self) -> Result<(), Differ> {
        self.token(Token::Close)
    }
}

impl ser::SerializeTupleVariant for &mut Walk<'_> {
    type Ok = ();
    type Error = Differ;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Differ> {
        value.serialize(&mut **self)
    }
    fn end(self) -> Result<(), Differ> {
        self.token(Token::Close)
    }
}

impl ser::SerializeMap for &mut Walk<'_> {
    type Ok = ();
    type Error = Differ;
    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<(), Differ> {
        key.serialize(&mut **self)
    }
    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Differ> {
        value.serialize(&mut **self)
    }
    fn end(self) -> Result<(), Differ> {
        self.token(Token::Close)
    }
}

impl ser::SerializeStruct for &mut Walk<'_> {
    type Ok = ();
    type Error = Differ;
    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        _: &'static str,
        value: &T,
    ) -> Result<(), Differ> {
        value.serialize(&mut **self)
    }
    fn skip_field(&mut self, _: &'static str) -> Result<(), Differ> {
        self.token(Token::Skipped)
    }
    fn end(self) -> Result<(), Differ> {
        self.token(Token::Close)
    }
}

impl ser::SerializeStructVariant for &mut Walk<'_> {
    type Ok = ();
    type Error = Differ;
    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        _: &'static str,
        value: &T,
    ) -> Result<(), Differ> {
        value.serialize(&mut **self)
    }
    fn skip_field(&mut self, _: &'static str) -> Result<(), Differ> {
        self.token(Token::Skipped)
    }
    fn end(self) -> Result<(), Differ> {
        self.token(Token::Close)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_is_16_bytes() {
        assert_eq!(std::mem::size_of::<Token>(), 16);
    }

    #[derive(Serialize)]
    struct Fields {
        #[serde(skip_serializing_if = "Option::is_none")]
        a: Option<u8>,
        b: Option<u8>,
    }

    #[test]
    fn a_field_left_out_is_not_the_next_one() {
        let first = Fields {
            a: Some(1),
            b: None,
        };
        let second = Fields {
            a: None,
            b: Some(1),
        };
        assert!(!same_except_prefix(&first, "a", &second, "b"));
        assert!(same_except_prefix(&second, "a", &second, "b"));
    }

    #[test]
    fn a_string_reads_as_another_once_its_prefix_is_renamed() {
        assert!(reads_as("a-1", "b-1", "a", "b"));
        assert!(reads_as("a-1/a-2", "b-1/b-2", "a", "b"));
        assert!(reads_as("plain", "plain", "a", "b"));
        assert!(!reads_as("a-1", "b-2", "a", "b"));
        assert!(!reads_as("b-1", "b-1", "a", "b"));
        assert!(!reads_as("a-1x", "b-1", "a", "b"));
        assert!(reads_as("df1-2-3", "df9-2-3", "df1-2", "df9-2"));
    }

    #[test]
    fn values_match_but_for_the_prefix_and_differ_otherwise() {
        let kept = (vec!["a-1".to_owned(), "a-2".to_owned()], 0.5f32, Some(3u8));
        let fresh = (vec!["b-1".to_owned(), "b-2".to_owned()], 0.5f32, Some(3u8));
        assert!(same_except_prefix(&kept, "a", &fresh, "b"));
        let other = (vec!["b-1".to_owned(), "b-3".to_owned()], 0.5f32, Some(3u8));
        assert!(!same_except_prefix(&kept, "a", &other, "b"));
        let moved = (vec!["b-1".to_owned(), "b-2".to_owned()], 0.25f32, Some(3u8));
        assert!(!same_except_prefix(&kept, "a", &moved, "b"));
        let longer = (
            vec!["b-1".to_owned(), "b-2".to_owned(), "b-3".to_owned()],
            0.5f32,
            Some(3u8),
        );
        assert!(!same_except_prefix(&kept, "a", &longer, "b"));
        // As in JSON, every number that is not finite is the same `null`.
        assert!(same_except_prefix(&f32::NAN, "a", &f32::INFINITY, "b"));
        assert!(!same_except_prefix(&0.0f32, "a", &-0.0f32, "b"));
    }
}
