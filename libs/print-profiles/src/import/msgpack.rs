/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! A MessagePack reader for the settings files other slicers export
//! (#565): the types those files use, into a tree of [`Value`]s.

use std::collections::BTreeMap;

/// One MessagePack value.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Bin(Vec<u8>),
    Array(Vec<Value>),
    Map(BTreeMap<String, Value>),
}

impl Value {
    /// The value under `key` of a map.
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Map(map) => map.get(key),
            _ => None,
        }
    }

    /// A number, integer or float.
    pub fn as_f64(&self) -> Option<f64> {
        match *self {
            Value::Int(i) => Some(i as f64),
            Value::Float(f) => Some(f),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match *self {
            Value::Bool(b) => Some(b),
            _ => None,
        }
    }
}

/// Why bytes are not MessagePack this reader knows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MsgpackError(pub String);

/// The value `bytes` holds, all of them.
pub fn decode(bytes: &[u8]) -> Result<Value, MsgpackError> {
    let mut reader = Reader { bytes, at: 0 };
    let value = reader.value(0)?;
    if reader.at != bytes.len() {
        return Err(MsgpackError(format!(
            "{} bytes left after the value",
            bytes.len() - reader.at
        )));
    }
    Ok(value)
}

/// How deep values may nest: past it the bytes are taken as not ours.
const MAX_DEPTH: usize = 64;

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], MsgpackError> {
        let end = self
            .at
            .checked_add(n)
            .filter(|&end| end <= self.bytes.len())
            .ok_or_else(|| MsgpackError(format!("cut short at byte {}", self.at)))?;
        let out = &self.bytes[self.at..end];
        self.at = end;
        Ok(out)
    }

    fn uint(&mut self, n: usize) -> Result<u64, MsgpackError> {
        Ok(self
            .take(n)?
            .iter()
            .fold(0u64, |acc, &b| (acc << 8) | u64::from(b)))
    }

    fn int(&mut self, n: usize) -> Result<i64, MsgpackError> {
        let raw = self.uint(n)?;
        let shift = 64 - 8 * n as u32;
        Ok(((raw << shift) as i64) >> shift)
    }

    fn string(&mut self, n: usize) -> Result<Value, MsgpackError> {
        let bytes = self.take(n)?;
        String::from_utf8(bytes.to_vec())
            .map(Value::Str)
            .map_err(|_| MsgpackError("a string is not UTF-8".to_owned()))
    }

    fn array(&mut self, n: usize, depth: usize) -> Result<Value, MsgpackError> {
        // Every value takes a byte at least.
        if n > self.bytes.len() - self.at {
            return Err(MsgpackError(format!("{n} values cannot fit")));
        }
        (0..n)
            .map(|_| self.value(depth + 1))
            .collect::<Result<_, _>>()
            .map(Value::Array)
    }

    fn map(&mut self, n: usize, depth: usize) -> Result<Value, MsgpackError> {
        if n > self.bytes.len() - self.at {
            return Err(MsgpackError(format!("{n} entries cannot fit")));
        }
        let mut map = BTreeMap::new();
        for _ in 0..n {
            let key = match self.value(depth + 1)? {
                Value::Str(s) => s,
                Value::Int(i) => i.to_string(),
                other => return Err(MsgpackError(format!("a key that is {other:?}"))),
            };
            let value = self.value(depth + 1)?;
            map.insert(key, value);
        }
        Ok(Value::Map(map))
    }

    fn value(&mut self, depth: usize) -> Result<Value, MsgpackError> {
        if depth > MAX_DEPTH {
            return Err(MsgpackError("values nest too deep".to_owned()));
        }
        let tag = self.take(1)?[0];
        Ok(match tag {
            0x00..=0x7f => Value::Int(i64::from(tag)),
            0x80..=0x8f => self.map(usize::from(tag & 0x0f), depth)?,
            0x90..=0x9f => self.array(usize::from(tag & 0x0f), depth)?,
            0xa0..=0xbf => self.string(usize::from(tag & 0x1f))?,
            0xc0 => Value::Nil,
            0xc2 => Value::Bool(false),
            0xc3 => Value::Bool(true),
            0xc4..=0xc6 => {
                let n = self.uint(1 << (tag - 0xc4))? as usize;
                Value::Bin(self.take(n)?.to_vec())
            }
            0xca => Value::Float(f64::from(f32::from_bits(self.uint(4)? as u32))),
            0xcb => Value::Float(f64::from_bits(self.uint(8)?)),
            0xcc..=0xcf => {
                let raw = self.uint(1 << (tag - 0xcc))?;
                Value::Int(
                    i64::try_from(raw).map_err(|_| MsgpackError("an integer too large".into()))?,
                )
            }
            0xd0..=0xd3 => Value::Int(self.int(1 << (tag - 0xd0))?),
            0xd9..=0xdb => {
                let n = self.uint(1 << (tag - 0xd9))? as usize;
                self.string(n)?
            }
            0xdc | 0xdd => {
                let n = self.uint(if tag == 0xdc { 2 } else { 4 })? as usize;
                self.array(n, depth)?
            }
            0xde | 0xdf => {
                let n = self.uint(if tag == 0xde { 2 } else { 4 })? as usize;
                self.map(n, depth)?
            }
            0xe0..=0xff => Value::Int(i64::from(tag as i8)),
            _ => {
                return Err(MsgpackError(format!(
                    "a type this reader does not know ({tag:#x})"
                )));
            }
        })
    }
}
