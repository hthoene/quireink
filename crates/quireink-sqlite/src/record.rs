//! The record format: a header of serial types, then the values.

use alloc::string::String;
use alloc::vec::Vec;
use core::cmp::Ordering;

use crate::varint;

/// A value of a column.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Int(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

impl Value {
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Value::Int(v) => Some(*v),
            Value::Real(v) => Some(*v as i64),
            _ => None,
        }
    }

    pub fn as_text(&self) -> Option<&str> {
        match self {
            Value::Text(t) => Some(t),
            _ => None,
        }
    }
}

/// The serial type of an integer and its size in bytes.
fn int_type(v: i64) -> (u64, usize) {
    match v {
        0 => (8, 0),
        1 => (9, 0),
        -128..=127 => (1, 1),
        -32_768..=32_767 => (2, 2),
        -8_388_608..=8_388_607 => (3, 3),
        -2_147_483_648..=2_147_483_647 => (4, 4),
        -140_737_488_355_328..=140_737_488_355_327 => (5, 6),
        _ => (6, 8),
    }
}

/// `values` as a record.
pub fn encode_record(values: &[Value]) -> Vec<u8> {
    let mut types = Vec::new();
    let mut body = Vec::new();
    for value in values {
        let serial = match value {
            Value::Null => 0,
            Value::Int(v) => {
                let (serial, size) = int_type(*v);
                body.extend_from_slice(&v.to_be_bytes()[8 - size..]);
                serial
            }
            Value::Real(v) => {
                body.extend_from_slice(&v.to_bits().to_be_bytes());
                7
            }
            Value::Text(t) => {
                body.extend_from_slice(t.as_bytes());
                t.len() as u64 * 2 + 13
            }
            Value::Blob(b) => {
                body.extend_from_slice(b);
                b.len() as u64 * 2 + 12
            }
        };
        varint::write(&mut types, serial);
    }
    // The header's size counts itself.
    let mut size = types.len() + 1;
    while varint::len(size as u64) + types.len() != size {
        size = varint::len(size as u64) + types.len();
    }
    let mut out = Vec::with_capacity(size + body.len());
    varint::write(&mut out, size as u64);
    out.extend_from_slice(&types);
    out.extend_from_slice(&body);
    out
}

/// The size of a value of serial type `serial`.
fn value_size(serial: u64) -> usize {
    match serial {
        0 | 8 | 9 | 10 | 11 => 0,
        1 => 1,
        2 => 2,
        3 => 3,
        4 => 4,
        5 => 6,
        6 | 7 => 8,
        n => ((n - 12) / 2) as usize,
    }
}

fn read_int(bytes: &[u8]) -> i64 {
    // Sign-extended big-endian.
    let mut v: i64 = if bytes.first().is_some_and(|b| b & 0x80 != 0) {
        -1
    } else {
        0
    };
    for &b in bytes {
        v = (v << 8) | i64::from(b);
    }
    v
}

/// The values of a record; `None` if it is malformed.
pub fn decode_record(data: &[u8]) -> Option<Vec<Value>> {
    let (header_size, mut at) = varint::read(data)?;
    let header_size = usize::try_from(header_size).ok()?;
    let mut body = header_size;
    let mut values = Vec::new();
    while at < header_size {
        let (serial, n) = varint::read(data.get(at..)?)?;
        at += n;
        let size = value_size(serial);
        let bytes = data.get(body..body + size)?;
        body += size;
        values.push(match serial {
            0 | 10 | 11 => Value::Null,
            1..=6 => Value::Int(read_int(bytes)),
            7 => Value::Real(f64::from_bits(read_int(bytes) as u64)),
            8 => Value::Int(0),
            9 => Value::Int(1),
            n if n % 2 == 1 => Value::Text(String::from_utf8_lossy(bytes).into_owned()),
            _ => Value::Blob(bytes.to_vec()),
        });
    }
    Some(values)
}

/// SQLite's order of values with the BINARY collation: NULL, numbers,
/// text (bytewise), blobs.
fn compare_values(a: &Value, b: &Value) -> Ordering {
    let class = |v: &Value| match v {
        Value::Null => 0,
        Value::Int(_) | Value::Real(_) => 1,
        Value::Text(_) => 2,
        Value::Blob(_) => 3,
    };
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => x.cmp(y),
        (Value::Int(x), Value::Real(y)) => (*x as f64).total_cmp(y),
        (Value::Real(x), Value::Int(y)) => x.total_cmp(&(*y as f64)),
        (Value::Real(x), Value::Real(y)) => x.total_cmp(y),
        (Value::Text(x), Value::Text(y)) => x.as_bytes().cmp(y.as_bytes()),
        (Value::Blob(x), Value::Blob(y)) => x.cmp(y),
        _ => class(a).cmp(&class(b)),
    }
}

/// The order of two index entries (records), column by column.
pub fn compare_records(a: &[u8], b: &[u8]) -> Ordering {
    match (decode_record(a), decode_record(b)) {
        (Some(a), Some(b)) => {
            for (x, y) in a.iter().zip(b.iter()) {
                let order = compare_values(x, y);
                if order != Ordering::Equal {
                    return order;
                }
            }
            a.len().cmp(&b.len())
        }
        _ => a.cmp(b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use alloc::vec;

    #[test]
    fn records_round_trip() {
        let values = vec![
            Value::Null,
            Value::Int(0),
            Value::Int(1),
            Value::Int(-5),
            Value::Int(300),
            Value::Int(70_000),
            Value::Int(1_760_000_000),
            Value::Int(1 << 40),
            Value::Int(i64::MIN),
            Value::Real(1.5),
            Value::Text("Fjord: ø".to_string()),
            Value::Blob(vec![1, 2, 3]),
        ];
        let data = encode_record(&values);
        assert_eq!(decode_record(&data), Some(values));
        // An empty record and a long header.
        assert_eq!(decode_record(&encode_record(&[])), Some(vec![]));
        let many = vec![Value::Int(7); 200];
        assert_eq!(decode_record(&encode_record(&many)), Some(many));
    }

    #[test]
    fn records_sort_like_sqlite() {
        let r = |values: &[Value]| encode_record(values);
        let text = |t: &str| Value::Text(t.to_string());
        assert_eq!(
            compare_records(
                &r(&[Value::Int(2), Value::Int(9)]),
                &r(&[Value::Int(10), Value::Int(1)])
            ),
            Ordering::Less
        );
        assert_eq!(
            compare_records(&r(&[Value::Null]), &r(&[Value::Int(-9)])),
            Ordering::Less
        );
        assert_eq!(
            compare_records(&r(&[Value::Int(9)]), &r(&[text("0")])),
            Ordering::Less
        );
        assert_eq!(
            compare_records(&r(&[text("B")]), &r(&[text("a")])),
            Ordering::Less
        );
        assert_eq!(
            compare_records(
                &r(&[text("a"), Value::Int(2)]),
                &r(&[text("a"), Value::Int(2)])
            ),
            Ordering::Equal
        );
    }
}
