use std::{self, slice, vec, result, char, error, io};
use std::str::{self, FromStr,CharIndices};
use std::fmt::{self, Write};
use std::ops::{Neg, Not, Mul, Div, Rem, Add, Sub, Shr, Shl};
use std::ops::{BitAnd, BitXor, BitOr};

use db::{Document,Doctype,ItemIterator};
use db::{Docindex,And,Or,Slice,Recurse,Append,Value};
use lex::Lex;
use prop;
use util;

include!("./json.rs.lookup");


pub type Result<T> = result::Result<T,Error>;


#[derive(Debug,Eq,PartialEq)]
pub enum Error {
    Parse(String),
    ParseFloat(std::num::ParseFloatError, String),
    ParseInt(std::num::ParseIntError, String),
    NotMyType(String),
    KeyMissing(usize, String),
    IndexUnbound(isize, isize),
}

impl Error {
    fn key_missing_at(&self) -> usize {
        match self { Error::KeyMissing(at, _) => *at, _ => unreachable!() }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use json::Error::*;

        match self {
            Parse(s) => write!(f, "{}", s),
            ParseFloat(err, s) => write!(f, "{}, {}", err, s),
            ParseInt(err, s) => write!(f, "{}, {}", err, s),
            NotMyType(s) => write!(f, "{}", s),
            KeyMissing(_, key) => write!(f, "missing key {}", key),
            IndexUnbound(s,e) => write!(f, "index out of bound {}..{}", s, e),
        }
    }
}

impl error::Error for Error {
    fn cause(&self) -> Option<&error::Error> {
        match self {
            Error::ParseFloat(err, _) => Some(err),
            Error::ParseInt(err, _) => Some(err),
            _ => None,
        }
    }
}

impl From<std::num::ParseFloatError> for Error {
    fn from(err: std::num::ParseFloatError) -> Error {
        Error::ParseFloat(err, String::new())
    }
}

impl From<std::num::ParseIntError> for Error {
    fn from(err: std::num::ParseIntError) -> Error {
        Error::ParseInt(err, String::new())
    }
}



pub struct JsonBuf {
    inner: String,
    lex: Lex,
}

impl JsonBuf {
    pub fn new() -> JsonBuf {
        JsonBuf{inner: String::new(), lex: Lex::new(0, 1, 1)}
    }

    pub fn with_capacity(cap: usize) -> JsonBuf {
        JsonBuf{inner: String::with_capacity(cap), lex: Lex::new(0, 1, 1)}
    }

    pub fn iter<R>(r: R) -> Jsons<R> where R: io::Read {
        Jsons::new(r)
    }

    pub fn set<T>(&mut self, text: &T) where T: AsRef<str> + ?Sized {
        self.inner.clear();
        self.lex.set(0, 1, 1);
        self.inner.push_str(text.as_ref());
    }

    pub fn append<T>(&mut self, text: &T) where T: AsRef<str> + ?Sized {
        self.inner.push_str(text.as_ref());
    }

    pub fn parse(&mut self) -> Result<Json> {
        self.lex.set(0, 1, 1);
        let val = parse_value(&self.inner, &mut self.lex)?;
        self.inner.drain(..self.lex.off);
        Ok(val)
    }
}

impl From<String> for JsonBuf {
    fn from(inner: String) -> JsonBuf {
        JsonBuf{inner, lex: Lex::new(0,1,1)}
    }
}

impl<'a, T> From<&'a T> for JsonBuf where T: AsRef<str> + ?Sized {
    fn from(s: &'a T) -> JsonBuf {
        JsonBuf::from(s.as_ref().to_string())
    }
}


pub struct Jsons<R> where R: io::Read {
    inner: String,
    lex: Lex,
    reader: R,
    buffer: Vec<u8>,
}

impl<R> Jsons<R> where R: io::Read {
    const BLOCK_SIZE: usize = 1024;

    fn new(reader: R) -> Jsons<R> {
        let inner = String::with_capacity(Self::BLOCK_SIZE);
        let mut buffer = Vec::with_capacity(Self::BLOCK_SIZE);
        let lex = Lex::new(0, 1, 1);
        unsafe{ buffer.set_len(Self::BLOCK_SIZE) };
        Jsons{ inner, lex, reader, buffer }
    }
}

impl<R> Iterator for Jsons<R> where R: io::Read {
    type Item=Json;

    fn next(&mut self) -> Option<Json> {
        // TODO: automatically adjust the cap/len of self.buffer.
        self.lex.set(0, 1, 1);
        loop {
            if let Ok(val) = parse_value(&self.inner, &mut self.lex) {
                self.inner.drain(..self.lex.off);
                return Some(val)
            }
            let v = match self.reader.read(&mut self.buffer) {
                Ok(0) | Err(_) => return None,
                Ok(n) => unsafe { str::from_utf8_unchecked(&self.buffer[..n]) },
            };
            self.inner.push_str(v);
        }
    }
}


fn parse_value(text: &str, lex: &mut Lex) -> Result<Json> {
    parse_whitespace(text, lex);
    check_eof(text, lex)?;

    //println!("text -- {:?}", valtext);
    let v = match (&text[lex.off..]).as_bytes()[0] {
        b'n' => parse_null(text, lex),
        b't' => parse_true(text, lex),
        b'f' => parse_false(text, lex),
        b'0'..=b'9'|b'+'|b'-'|b'.'|b'e'|b'E' => parse_num(text, lex),
        b'"' => parse_string(text, lex),
        b'[' => parse_array(text, lex),
        b'{' => parse_object(text, lex),
        ch => {
            Err(Error::Parse(lex.format(&format!("invalid token {}", ch))))
        }
    };
    //println!("valu -- {:?}", v);

    // gather up lexical position for a subset of error-variants.
    match v {
        Err(Error::ParseFloat(e, _)) => {
            Err(Error::ParseFloat(e, lex.format("invalid float")))
        },

        Err(Error::ParseInt(e, _)) => {
            Err(Error::ParseInt(e, lex.format("invalid integer")))
        }

        rc => rc,
    }
}

fn parse_null(text: &str, lex: &mut Lex) -> Result<Json> {
    let text = &text[lex.off..];
    if text.len() >= 4 && &text[..4] == "null" {
        lex.incr_col(4);
        Ok(Json::Null)
    } else {
        Err(Error::Parse(lex.format("expected null")))
    }
}

fn parse_true(text: &str, lex: &mut Lex) -> Result<Json> {
    let text = &text[lex.off..];
    if text.len() >= 4 && &text[..4] == "true" {
        lex.incr_col(4);
        Ok(Json::Bool(true))
    } else {
        Err(Error::Parse(lex.format("expected true")))
    }
}

fn parse_false(text: &str, lex: &mut Lex) -> Result<Json> {
    let text = &text[lex.off..];
    if text.len() >= 5 && &text[..5] == "false" {
        lex.incr_col(5);
        Ok(Json::Bool(false))
    } else {
        Err(Error::Parse(lex.format("expected false")))
    }
}

fn parse_num(text: &str, lex: &mut Lex) -> Result<Json> {
    let text = &text[lex.off..];
    let mut doparse = |text: &str, i: usize, is_float: bool| -> Result<Json> {
        lex.incr_col(i);
        if is_float {
            let val = text.parse::<f64>()?;
            Ok(Json::Float(val))
        } else {
            let val = text.parse::<i128>()?;
            Ok(Json::Integer(val))
        }
    };

    let mut is_float = false;
    for (i, ch) in text.char_indices() {
        match ch {
            '0'..='9'| '+'| '-' => continue, // valid number
            '.' | 'e' | 'E' => { is_float = true; continue}, // float number
            _ => (),
        }
        return doparse(&text[..i], i, is_float)
    }
    doparse(text, text.len(), is_float)
}

pub(super) fn parse_string(text: &str, lex: &mut Lex) -> Result<Json> {
    use self::Json::{String as S};

    let mut escape = false;
    let mut res = String::new();
    let mut chars = (&text[lex.off..]).char_indices();

    let (i, ch) = chars.next().unwrap(); // skip the opening quote
    if ch != '"' {
        return Err(Error::Parse(lex.format("not a string")))
    }

    while let Some((i, ch)) = chars.next() {
        if escape == false {
            if ch == '\\' {
                escape = true;
                continue
            }
            match ch {
                '"' => {
                    lex.incr_col(i+1);
                    return Ok(S(res));
                },
                _ => res.push(ch),
            }
            continue
        }

        // previous char was escape
        match ch {
            '"' => res.push('"'),
            '\\' => res.push('\\'),
            '/' => res.push('/'),
            'b' => res.push('\x08'),
            'f' => res.push('\x0c'),
            'n' => res.push('\n'),
            'r' => res.push('\r'),
            't' => res.push('\t'),
            'u' => match decode_json_hex_code(&mut chars, lex)? {
                code1 @ 0xDC00 ... 0xDFFF => {
                    lex.incr_col(i);
                    let err = format!("invalid string codepoint {}", code1);
                    return Err(Error::Parse(lex.format(&err)))
                },
                // Non-BMP characters are encoded as a sequence of
                // two hex escapes, representing UTF-16 surrogates.
                code1 @ 0xD800 ... 0xDBFF => {
                    let code2 = decode_json_hex_code2(&mut chars, lex)?;
                    if code2 < 0xDC00 || code2 > 0xDFFF {
                        lex.incr_col(i);
                        let err = format!("invalid string codepoint {}", code2);
                        return Err(Error::Parse(lex.format(&err)))
                    }
                    let code = (((code1 - 0xD800) as u32) << 10 |
                                 (code2 - 0xDC00) as u32) + 0x1_0000;
                    res.push(char::from_u32(code).unwrap());
                },

                n => match char::from_u32(n as u32) {
                    Some(ch) => res.push(ch),
                    None => {
                        lex.incr_col(i);
                        let err = format!("invalid string escape code {:?}", n);
                        return Err(Error::Parse(lex.format(&err)))
                    },
                },
            },
            _ => {
                lex.incr_col(i);
                let err = "invalid string string escape type";
                return Err(Error::Parse(lex.format(&err)))
            },
        }
        escape = false;
    }
    lex.incr_col(i);
    return Err(Error::Parse(lex.format("incomplete string")))
}

fn decode_json_hex_code(chars: &mut CharIndices, lex: &mut Lex)
    -> Result<u32>
{
    let mut n = 0;
    let mut code = 0_u32;
    while let Some((_, ch)) = chars.next() {
        if (ch as u8) > 128 || HEXNUM[ch as usize] == 20 {
            let err = format!("invalid string escape code {:?}", ch);
            return Err(Error::Parse(lex.format(&err)))
        }
        code = code * 16 + (HEXNUM[ch as usize] as u32);
        n += 1;
        if n == 4 {
            break
        }
    }
    if n != 4 {
        let err = format!("incomplete string escape code {:x}", code);
        return Err(Error::Parse(lex.format(&err)))
    }
    Ok(code)
}

fn decode_json_hex_code2(chars: &mut CharIndices, lex: &mut Lex)
    -> Result<u32>
{
    if let Some((_, ch1)) = chars.next() {
        if let Some((_, ch2)) = chars.next() {
            if ch1 == '\\' && ch2 == 'u' {
                return decode_json_hex_code(chars, lex)
            }
        }
    }
    let err = "invalid string string escape type";
    return Err(Error::Parse(lex.format(err)))
}


pub(super) fn parse_array(text: &str, lex: &mut Lex) -> Result<Json> {
    lex.incr_col(1); // skip '['

    let mut array: Vec<Json> = Vec::new();
    parse_whitespace(text, lex);
    if (&text[lex.off..]).as_bytes()[0] == b',' {
        return Err(Error::Parse(lex.format("expected ','")))
    }
    loop {
        if (&text[lex.off..]).as_bytes()[0] == b']' { // end of array.
            lex.incr_col(1);
            break Ok(Json::Array(array))
        }

        array.push(parse_value(text, lex)?);

        parse_whitespace(text, lex);
        if (&text[lex.off..]).as_bytes()[0] == b',' { // skip comma
            lex.incr_col(1);
            parse_whitespace(text, lex);
        }
    }
}

pub(super) fn parse_object(text: &str, lex: &mut Lex) -> Result<Json> {
    lex.incr_col(1); // skip '{'

    let mut m: Vec<Property> = Vec::new();
    parse_whitespace(text, lex);
    if (&text[lex.off..]).as_bytes()[0] == b'}' {
        lex.incr_col(1);
        return Ok(Json::Object(m))
    }
    loop {
        // key
        parse_whitespace(text, lex);
        let key: String = parse_string(text, lex)?.string().unwrap();
        // colon
        parse_whitespace(text, lex);
        check_next_byte(text, lex, b':')?;

        // value
        parse_whitespace(text, lex);
        let value = parse_value(text, lex)?;

        Json::insert(&mut m, Property::new(key, value));
        //println!("parse {} {} {:?}", key, i, m);

        // is exit
        parse_whitespace(text, lex);
        if (&text[lex.off..]).len() == 0 {
            break Err(Error::Parse(lex.format("unexpected eof")))
        } else if (&text[lex.off..]).as_bytes()[0] == b'}' { // exit
            lex.incr_col(1);
            break Ok(Json::Object(m))
        } else if (&text[lex.off..]).as_bytes()[0] == b',' { // skip comma
            lex.incr_col(1);
        }
    }
}

fn parse_whitespace(text: &str, lex: &mut Lex) {
    for &ch in (&text[lex.off..]).as_bytes() {
        match WS_LOOKUP[ch as usize] {
            0 => break,
            1 => { lex.col += 1 },              // ' ' | '\t' | '\r'
            2 => { lex.row += 1; lex.col = 0 }, // '\n'
            _ => panic!("unreachable code"),
        };
        lex.off += 1;
    }
}

fn check_next_byte(text: &str, lex: &mut Lex, b: u8) -> Result<()> {
    let progbytes = (&text[lex.off..]).as_bytes();

    if progbytes.len() == 0 {
        return Err(Error::Parse(lex.format(&format!("missing token {}", b))));
    }

    if progbytes[0] != b {
        return Err(Error::Parse(lex.format(&format!("invalid token {}", b))));
    }
    lex.incr_col(1);

    Ok(())
}

fn check_eof(text: &str, lex: &mut Lex) -> Result<()> {
    if (&text[lex.off..]).len() == 0 {
        Err(Error::Parse(lex.format("unexpected eof")))

    } else {
        Ok(())
    }
}


pub type Property = prop::Property<Json>;


#[derive(Clone,PartialEq,PartialOrd)]
pub enum Json {
    Null,
    Bool(bool),
    Integer(i128),
    Float(f64),
    String(String),
    Array(Vec<Json>),
    Object(Vec<Property>),
}

impl Json {
    fn encode_string<W: Write>(w: &mut W, val: &str) -> fmt::Result {
        write!(w, "\"")?;

        let mut start = 0;
        for (i, byte) in val.bytes().enumerate() {
            let escstr = ESCAPE[byte as usize];
            if escstr.len() == 0 { continue }

            if start < i {
                write!(w, "{}", &val[start..i])?;
            }
            write!(w, "{}", escstr)?;
            start = i + 1;
        }
        if start != val.len() {
            write!(w, "{}", &val[start..])?;
        }
        write!(w, "\"")
    }

    fn insert(props: &mut Vec<Property>, prop: Property) {
        match search_by_key(props, prop.key_ref()) {
            Ok(off) => props[off] = prop,
            Err(err) => props.insert(err.key_missing_at(), prop),
        };
    }
}

impl Default for Json {
    fn default() -> Json {
        Json::Null
    }
}

impl From<bool> for Json {
    fn from(val: bool) -> Json {
        Json::Bool(val)
    }
}

impl From<i128> for Json {
    fn from(val: i128) -> Json {
        Json::Integer(val)
    }
}

impl From<f64> for Json {
    fn from(val: f64) -> Json {
        Json::Float(val)
    }
}

impl From<String> for Json {
    fn from(val: String) -> Json {
        Json::String(val)
    }
}

impl From<Vec<Json>> for Json {
    fn from(val: Vec<Json>) -> Json {
        Json::Array(val)
    }
}

impl From<Vec<Property>> for Json {
    fn from(vals: Vec<Property>) -> Json {
        let mut props: Vec<Property> = Vec::with_capacity(vals.len());
        vals.into_iter().for_each(|val| Json::insert(&mut props, val));
        Json::Object(props)
    }
}

impl From<Json> for bool {
    fn from(val: Json) -> bool {
        match val { Json::Null | Json::Bool(false) => false, _ => true }
    }
}

impl FromStr for Json {
    type Err=Error;

    fn from_str(text: &str) -> Result<Json> {
        let mut lex = Lex::new(0, 1, 1);
        parse_value(&text, &mut lex)
    }
}

impl fmt::Display for Json {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use json::Json::{Null,Bool,Integer,Float,Array,Object, String as S};

        match self {
            Null => write!(f, "null"),
            Bool(true) => write!(f, "true"),
            Bool(false) => write!(f, "false"),
            Integer(val) => write!(f, "{}", val),
            Float(val) => write!(f, "{:e}", val),
            S(val) => { Self::encode_string(f, &val)?; Ok(()) },
            Array(val) => {
                if val.len() == 0 {
                    write!(f, "[]")

                } else {
                    write!(f, "[")?;
                    for item in val[..val.len()-1].iter() {
                        write!(f, "{},", item)?;
                    }
                    write!(f, "{}", val[val.len()-1])?;
                    write!(f, "]")
                }
            },
            Object(val) => {
                let val_len = val.len();
                if val_len == 0 {
                    write!(f, "{{}}")

                } else {
                    write!(f, "{{")?;
                    for (i, kv) in val.iter().enumerate() {
                        Self::encode_string(f, kv.key_ref())?;
                        write!(f, ":{}", kv.value_ref())?;
                        if i < (val_len - 1) { write!(f, ",")?; }
                    }
                    write!(f, "}}")
                }
            }
        }
    }
}

impl fmt::Debug for Json {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        <Self as fmt::Display>::fmt(self, f)
    }
}

impl Document for Json {
    fn doctype(&self) -> Doctype {
        match self {
            Json::Null => Doctype::Null,
            Json::Bool(_) => Doctype::Bool,
            Json::Integer(_) => Doctype::Integer,
            Json::Float(_) => Doctype::Float,
            Json::String(_) => Doctype::String,
            Json::Array(_) => Doctype::Array,
            Json::Object(_) => Doctype::Object,
        }
    }

    fn len(&self) -> Option<usize> {
        match self {
            Json::String(s) => Some(s.len()),
            Json::Array(a) => Some(a.len()),
            Json::Object(o) => Some(o.len()),
            Json::Null => Some(0),
            _ => None,
        }
    }

    fn set(&mut self, key: &str, value: Json) {
        match self {
            Json::Object(obj) => {
                Json::insert(obj, Property::new(key.to_string(), value));
            },
            _ => panic!("cannot set {:?} with {}", self.doctype(), key),
        }
    }
}

impl Value for Json {
    type item=Json;

    fn null() -> Json {
        Json::Null
    }

    fn boolean(self) -> Option<bool> {
        match self { Json::Bool(s) => Some(s), _ => None }
    }

    fn string_ref(&self) -> Option<&String> {
        match self { Json::String(s) => Some(s), _ => None }
    }

    fn string(self) -> Option<String> {
        match self { Json::String(s) => Some(s), _ => None }
    }

    fn integer(self) -> Option<i128> {
        match self { Json::Integer(n) => Some(n), _ => None }
    }

    fn float(self) -> Option<f64> {
        match self { Json::Float(f) => Some(f), _ => None }
    }

    fn array_ref(&self) -> Option<&Vec<Json>> {
        match self { Json::Array(arr) => Some(arr), _ => None }
    }

    fn array(self) -> Option<Vec<Json>> {
        match self { Json::Array(arr) => Some(arr), _ => None }
    }

    fn object_ref(&self) -> Option<&Vec<Property>> {
        match self { Json::Object(obj) => Some(obj), _ => None }
    }

    fn object(self) -> Option<Vec<Property>> {
        match self { Json::Object(obj) => Some(obj), _ => None }
    }
}

impl Recurse for Json {
    type item=Json;

    fn recurse(self) -> Vec<Json> {
        let mut list = Vec::new();
        do_recurse(self, &mut list);
        list
    }
}

pub fn do_recurse(value: Json, list: &mut Vec<Json>) {
    use self::Json::{Array, Object};

    match value {
        Array(values) => {
            list.push(Array(values.clone()));
            values.into_iter().for_each(|value| do_recurse(value, list));
        },
        Object(props) => {
            list.push(Object(props.clone()));
            props.into_iter().for_each(|prop| do_recurse(prop.value(), list));
        },
        doc => list.push(doc),
    }
}

impl Docindex<isize> for Json {
    type item=Json;

    fn index(self, off: isize) -> Option<Json> {
        match self {
            Json::Array(a) => {
                Some(a[util::normalized_offset(off, a.len())?].clone())
            }
            _ => None
        }
    }

    fn index_ref(&self, off: isize) -> Option<&Json> {
        match self {
            Json::Array(a) => {
                Some(&a[util::normalized_offset(off, a.len())?])
            }
            _ => None
        }
    }

    fn index_mut(&mut self, off: isize) -> Option<&mut Json> {
        match self {
            Json::Array(a) => {
                let off = util::normalized_offset(off, a.len())?;
                Some(&mut a[off])
            },
            _ => None
        }
    }

    fn get<'a>(self, key: &'a str) -> Option<Json> {
        match self {
            Json::Object(mut obj) => {
                let off = search_by_key(&obj, key).ok()?;
                Some(obj.remove(off).value())
            },
            _ => None
        }
    }

    fn get_ref<'a>(&self, key: &'a str) -> Option<&Json> {
        match self {
            Json::Object(obj) => {
                let off = search_by_key(obj, key).ok()?;
                Some(obj[off].value_ref())
            }
            _ => None,
        }
    }

    fn get_mut<'a>(&mut self, key: &'a str) -> Option<&mut Json> {
        match self {
            Json::Object(obj) => {
                let off = search_by_key(obj, key).ok()?;
                Some(obj[off].value_mut())
            }
            _ => None,
        }
    }
}

impl ItemIterator<Json> for Json {
    fn iter(&self) -> Option<slice::Iter<Json>> {
        match self {
            Json::Array(arr) => Some(arr.iter()),
            _ => None
        }
    }

    fn into_iter(self) -> Option<vec::IntoIter<Json>> {
        match self {
            Json::String(s) => {
                let out: Vec<Json> = s.chars().into_iter()
                    .map(|x| Json::Integer(x as i128))
                    .collect();
                Some(out.into_iter())
            },
            Json::Array(arr) => Some(arr.into_iter()),
            _ => None
        }
    }
}

impl ItemIterator<Property> for Json {
    fn iter(&self) -> Option<slice::Iter<Property>> {
        match self {
            Json::Object(obj) => Some(obj.iter()),
            _ => None
        }
    }

    fn into_iter(self) -> Option<vec::IntoIter<Property>> {
        match self {
            Json::Object(obj) => Some(obj.into_iter()),
            _ => None
        }
    }
}

impl Slice for Json {
    type item=Json;

    fn slice(self, start: isize, end: isize) -> Option<Json> {
        match self {
            Json::Array(arr) => {
                let (a, z) = util::slice_range_check(start, end, arr.len())?;
                Some(Json::Array(arr[a..z].to_vec()))
            },
            Json::String(s) => {
                let (a, z) = util::slice_range_check(start, end, s.len())?;
                Some(Json::String(s[a..z].to_string()))
            },
            _ => None,
        }
    }
}

impl Append<String> for Json {
    fn append(&mut self, value: String) {
        match self {
            Json::String(s) => s.push_str(&value),
            _ => panic!("cannot append to {:?}", self.doctype()),
        }
    }
}

impl Append<Vec<Json>> for Json {
    fn append(&mut self, values: Vec<Json>) {
        match self {
            Json::Array(arr) => {
                values.into_iter().for_each(|val| arr.push(val))
            },
            _ => panic!("cannot append to {:?}", self.doctype()),
        }
    }
}

impl Append<Vec<Property>> for Json {
    fn append(&mut self, properties: Vec<Property>) {
        match self {
            Json::Object(props) => {
                for prop in properties.into_iter() {
                    Json::insert(props, prop)
                }
            }
            _ => panic!("cannot append to {:?}", self.doctype()),
        }
    }
}


impl Neg for Json {
    type Output=Json;

    fn neg(self) -> Json {
        match self {
            Json::Integer(n) => Json::Integer(-n),
            Json::Float(n) => Json::Float(-n),
            _ => Json::Null,
        }
    }
}

impl Not for Json {
    type Output=Json;

    fn not(self) -> Json {
        let val: bool = From::from(self);
        Json::Bool(!val)
    }
}

impl Mul for Json {
    type Output=Json;

    fn mul(self, rhs: Json) -> Json {
        use json::Json::{Null,Integer,Float,Object, String as S};

        match (self, rhs) {
            (Integer(l), Integer(r)) => Integer(l*r),
            (Integer(l), Float(r)) => Float((l as f64) * r),
            (lhs@Integer(_), rhs) => rhs.mul(lhs),
            (Float(l), Float(r)) => Float(l*r),
            (Float(l), Integer(r)) => Float(l*(r as f64)),
            (lhs@Float(_), rhs) => rhs.mul(lhs),
            (S(_), Integer(0)) => Null,
            (S(s), Integer(n)) => S(s.repeat(n as usize)),
            (Object(this), Object(other)) => {
                let mut obj = Vec::new();
                obj = mixin_object(obj, this.to_vec());
                obj = mixin_object(obj, other.to_vec());
                Json::Object(obj)
            },
            (_, _) => Null,
        }
    }
}

impl Div for Json {
    type Output=Json;

    fn div(self, rhs: Json) -> Json {
        use json::Json::{Null,Integer,Float,String as S};

        match (self, rhs) {
            (Integer(_), Integer(0)) => Null,
            (Integer(_), Float(f)) if f == 0_f64 => Null,
            (Float(_), Integer(0)) => Null,
            (Float(_), Float(f)) if f == 0_f64 => Null,
            (Integer(l), Integer(r)) => Float((l as f64)/(r as f64)),
            (Integer(l), Float(r)) => Float((l as f64)/r),
            (Float(l), Float(r)) => Float(l/r),
            (Float(l), Integer(r)) => Float(l/(r as f64)),
            (S(s), S(patt)) => {
                let arr = s.split(&patt).map(|s| S(s.to_string())).collect();
                Json::Array(arr)
            },
            (_, _) => Null,
        }
    }
}

impl Rem for Json {
    type Output=Json;

    fn rem(self, rhs: Json) -> Json {
        use json::Json::{Null,Integer,Float};

        match (self, rhs) {
            (Integer(_), Integer(0)) => Null,
            (Integer(l), Integer(r)) => Integer(l%r),
            (Integer(_), Float(f)) if f == 0_f64 => Null,
            (Integer(l), Float(r)) => Float((l as f64)%r),
            (Float(_), Integer(0)) => Null,
            (Float(l), Integer(r)) => Float(l%(r as f64)),
            (Float(_), Float(f)) if f == 0_f64 => Null,
            (Float(l), Float(r)) => Float(l%r),
            (_, _) => Null,
        }
    }
}

impl Add for Json {
    type Output=Json;

    fn add(self, rhs: Json) -> Json {
        use json::Json::{Null,Integer,Float,Array,Object, String as S};

        match (self, rhs) {
            (Integer(l), Integer(r)) => Integer(l+r),
            (Integer(l), Float(r)) => Float((l as f64)+r),
            (lhs@Integer(_), rhs) => rhs.add(lhs),
            (Float(l), Float(r)) => Float(l+r),
            (Float(l), Integer(r)) => Float(l+(r as f64)),
            (lhs@Float(_), rhs) => rhs.add(lhs),
            (S(l), S(r)) => {
                let mut s = String::new(); s.push_str(&l); s.push_str(&r);
                S(s)
            }
            (Array(l), Array(r)) => {
                let mut a = Vec::with_capacity(l.len() + r.len());
                a.extend_from_slice(&l);
                a.extend_from_slice(&r);
                Array(a)
            }
            (Object(l), Object(r)) => {
                let mut obj = Vec::new();
                l.to_vec().into_iter().for_each(|p| Json::insert(&mut obj, p));
                r.to_vec().into_iter().for_each(|p| Json::insert(&mut obj, p));
                Json::Object(obj)
            }
            (_, _) => Null,
        }
    }
}

impl Sub for Json {
    type Output=Json;

    fn sub(self, rhs: Json) -> Json {
        use json::Json::{Null,Integer,Float,Array};

        match (self, rhs) {
            (Integer(l), Integer(r)) => Integer(l-r),
            (Integer(l), Float(r)) => Float((l as f64)-r),
            (lhs@Integer(_), rhs) => rhs.sub(lhs),
            (Float(l), Float(r)) => Float(l-r),
            (Float(l), Integer(r)) => Float(l-(r as f64)),
            (lhs@Float(_), rhs) => rhs.sub(lhs),
            (Array(mut lhs), Array(rhs)) => {
                rhs.iter().for_each(|x| {lhs.remove_item(x);});
                Array(lhs)
            },
            (_, _) => Null,
        }
    }
}

impl Shr for Json {
    type Output=Json;

    fn shr(self, rhs: Json) -> Json {
        match (self, rhs) {
            (Json::Integer(l), Json::Integer(r)) => Json::Integer(l>>r),
            (_, _) => Json::Null,
        }
    }
}

impl Shl for Json {
    type Output=Json;

    fn shl(self, rhs: Json) -> Json {
        match (self, rhs) {
            (Json::Integer(l), Json::Integer(r)) => Json::Integer(l<<r),
            (_, _) => Json::Null,
        }
    }
}

impl BitAnd for Json {
    type Output=Json;

    fn bitand(self, rhs: Json) -> Json {
        match (self, rhs) {
            (Json::Integer(l), Json::Integer(r)) => Json::Integer(l&r),
            (_, _) => Json::Null,
        }
    }
}

impl BitXor for Json {
    type Output=Json;

    fn bitxor(self, rhs: Json) -> Json {
        match (self, rhs) {
            (Json::Integer(l), Json::Integer(r)) => Json::Integer(l^r),
            (_, _) => Json::Null,
        }
    }
}

impl BitOr for Json {
    type Output=Json;

    fn bitor(self, rhs: Json) -> Json {
        match (self, rhs) {
            (Json::Integer(l), Json::Integer(r)) => Json::Integer(l|r),
            (_, _) => Json::Null,
        }
    }
}

impl And for Json {
    type Output=Json;

    fn and(self, other: Json) -> Json {
        let lhs: bool = From::from(self);
        let rhs: bool = From::from(other);
        From::from(lhs && rhs)
    }
}

impl Or for Json {
    type Output=Json;

    fn or(self, other: Json) -> Json {
        let lhs: bool = From::from(self);
        let rhs: bool = From::from(other);
        From::from(lhs || rhs)
    }
}


fn search_by_key(obj: &Vec<Property>, key: &str) -> Result<usize> {
    match prop::search_by_key(obj, key) {
        Ok(off) => Ok(off),
        Err(off) => Err(Error::KeyMissing(off, key.to_string())),
    }
}

// TODO: this logic can be simplified
fn mixin_object(mut this: Vec<Property>, other: Vec<Property>)
    -> Vec<Property>
{
    use json::Json::{Object};
    use json::Error::{KeyMissing};

    for o in other.into_iter() {
        match search_by_key(&this, o.key_ref()) {
            Ok(i) => match (this[i].clone().value(), o.clone().value()) {
                (Object(val), Object(val2)) => {
                    this[i].set_value(Object(mixin_object(val, val2)))
                },
                _ => this.insert(i, o)
            },
            Err(KeyMissing(i, _)) => this.insert(i, o.clone()),
            _ => unreachable!(),
        }
    }
    this
}


#[cfg(test)]
mod tests {
    use super::*;
    use test::Bencher;

    #[test]
    fn test_simple_jsons() {
        use self::Json::{Null, Bool, String, Integer, Float, Array, Object};

        let jsons = include!("../testdata/test_simple.jsons");
        let mut refs = include!("../testdata/test_simple.jsons.ref");
        let refs_len = refs.len();
        let mut jsonbuf = JsonBuf::new();

        let mut n = 4;
        let obj = Vec::new();
        refs[refs_len - n] = Object(obj);
        n -= 1;

        let mut obj = Vec::new();
        let (k, v) = ("key1".to_string(), r#""value1""#.parse().unwrap());
        obj.insert(0, Property::new(k, v));
        refs[refs_len - n] = Object(obj);
        n -= 1;

        let mut obj = Vec::new();
        let (k, v) = ("key1".to_string(), r#""value1""#.parse().unwrap());
        obj.insert(0, Property::new(k, v));
        let (k, v) = ("key2".to_string(), r#""value2""#.parse().unwrap());
        obj.insert(1, Property::new(k, v));
        refs[refs_len - n] = Object(obj);
        n -= 1;

        let mut obj = Vec::new();
        let (k, v) = ("a".to_string(), "1".parse().unwrap());
        obj.insert(0, Property::new(k, v));
        let (k, v) = ("b".to_string(), "1".parse().unwrap());
        obj.insert(1, Property::new(k, v));
        let (k, v) = ("c".to_string(), "1".parse().unwrap());
        obj.insert(2, Property::new(k, v));
        let (k, v) = ("d".to_string(), "1".parse().unwrap());
        obj.insert(3, Property::new(k, v));
        let (k, v) = ("e".to_string(), "1".parse().unwrap());
        obj.insert(4, Property::new(k, v));
        let (k, v) = ("f".to_string(), "1".parse().unwrap());
        obj.insert(5, Property::new(k, v));
        let (k, v) = ("x".to_string(), "1".parse().unwrap());
        obj.insert(6, Property::new(k, v));
        let (k, v) = ("z".to_string(), "1".parse().unwrap());
        obj.insert(7, Property::new(k, v));
        refs[refs_len - n] = Object(obj);

        jsonbuf.set(jsons[51]);
        let value = jsonbuf.parse().unwrap();
        assert_eq!(value, refs[51]);

        let ref_jsons = include!("../testdata/test_simple.jsons.ref.jsons");
        for (i, r) in refs.iter().enumerate() {
            let s = format!("{}", r);
            //println!("{} {}", i, &s);
            assert_eq!(&s, ref_jsons[i], "testcase: {}", i);
        }
    }

    #[test]
    fn test_json_iter() {
        use self::Json::{Integer, Float, Bool, Array, Object, String as S};

        let docs = r#"null 10 10.2 "hello world" true false [1,2] {"a":10}"#;
        let docs: &[u8] = docs.as_ref();
        let mut iter = JsonBuf::iter(docs);
        assert_eq!(Some(Json::Null), iter.next());
        assert_eq!(Some(Integer(10)), iter.next());
        assert_eq!(Some(Float(10.2)), iter.next());
        assert_eq!(Some(S("hello world".to_string())), iter.next());
        assert_eq!(Some(Bool(true)), iter.next());
        assert_eq!(Some(Bool(false)), iter.next());
        assert_eq!(
            Some(Array(vec![Integer(1), Integer(2), ])),
            iter.next()
        );
        assert_eq!(
            Some(Object(vec![Property::new("a".to_string(), Integer(10))])),
            iter.next(),
        );
    }

    #[bench]
    fn bench_null(b: &mut Bencher) {
        b.iter(|| {"null".parse::<Json>().unwrap()});
    }

    #[bench]
    fn bench_bool(b: &mut Bencher) {
        b.iter(|| {"false".parse::<Json>().unwrap()});
    }

    #[bench]
    fn bench_num(b: &mut Bencher) {
        b.iter(|| {"10.2".parse::<Json>().unwrap()});
    }

    #[bench]
    fn bench_string(b: &mut Bencher) {
        let s = r#""汉语 / 漢語; Hàn\b \tyǔ ""#;
        b.iter(|| {s.parse::<Json>().unwrap()});
    }

    #[bench]
    fn bench_array(b: &mut Bencher) {
            let s = r#" [null,true,false,10,"tru\"e"]"#;
        b.iter(|| {s.parse::<Json>().unwrap()});
    }

    #[bench]
    fn bench_map(b: &mut Bencher) {
        let s = r#"{"a":null,"b":true,"c":false,"d\"":-10E-1,"e":"tru\"e"}"#;
        b.iter(|| {s.parse::<Json>().unwrap()});
    }

    #[bench]
    fn bench_map_nom(b: &mut Bencher) {
        let s = r#"  { "a": 42, "b": ["x","y",12 ] , "c": {"hello":"world"}} "#;
        b.iter(|| {s.parse::<Json>().unwrap()});
    }

    #[bench]
    fn bench_null_to_json(b: &mut Bencher) {
        let val = "null".parse::<Json>().unwrap();
        let mut outs = String::with_capacity(64);
        b.iter(|| {outs.clear(); write!(outs, "{}", val)});
    }

    #[bench]
    fn bench_bool_to_json(b: &mut Bencher) {
        let val = "false".parse::<Json>().unwrap();
        let mut outs = String::with_capacity(64);
        b.iter(|| {outs.clear(); write!(outs, "{}", val)});
    }

    #[bench]
    fn bench_num_to_json(b: &mut Bencher) {
        let val = "10.2".parse::<Json>().unwrap();
        let mut outs = String::with_capacity(64);
        b.iter(|| {outs.clear(); write!(outs, "{}", val)});
    }

    #[bench]
    fn bench_string_to_json(b: &mut Bencher) {
        let inp = r#""汉语 / 漢語; Hàn\b \tyǔ ""#;
        let val = inp.parse::<Json>().unwrap();
        let mut outs = String::with_capacity(64);
        b.iter(|| {outs.clear(); write!(outs, "{}", val)});
    }

    #[bench]
    fn bench_array_to_json(b: &mut Bencher) {
            let inp = r#" [null,true,false,10,"tru\"e"]"#;
        let val = inp.parse::<Json>().unwrap();
        let mut outs = String::with_capacity(64);
        b.iter(|| {outs.clear(); write!(outs, "{}", val)});
    }

    #[bench]
    fn bench_map_to_json(b: &mut Bencher) {
        let inp = r#"{"a":null,"b":true,"c":false,"d\"":-10E-1,"e":"tru\"e"}"#;
        let val = inp.parse::<Json>().unwrap();
        let mut outs = String::with_capacity(64);
        b.iter(|| {outs.clear(); write!(outs, "{}", val)});
    }
}
