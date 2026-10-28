//! ISO 8601 for the calendar structs Torque ships `Torque.Encoder`
//! implementations for, written in the NIF so payloads holding them encode in
//! one pass instead of failing with `:unhandled_struct` and retrying after an
//! Elixir-side normalize walk.
//!
//! The output matches `Date.to_iso8601/1`, `Time.to_iso8601/1`,
//! `NaiveDateTime.to_iso8601/1` and `DateTime.to_iso8601/1` byte for byte.
//! Anything those functions would convert, reject or raise on (another
//! calendar, out-of-range fields, bignums, missing keys) is declined instead,
//! and takes the protocol path, which produces exactly what Elixir does.

use std::mem::MaybeUninit;

use rustler::sys::ERL_NIF_TERM;
use rustler::sys::{
    enif_get_int64, enif_get_map_value, enif_inspect_binary, ErlNifBinary, ErlNifEnv,
};

use crate::atoms;

/// The value under `key` in `map`, or `None` when absent.
#[inline]
pub(crate) fn map_get(
    env_raw: *mut ErlNifEnv,
    map: ERL_NIF_TERM,
    key: ERL_NIF_TERM,
) -> Option<ERL_NIF_TERM> {
    let mut out = MaybeUninit::<ERL_NIF_TERM>::uninit();
    // SAFETY: `map` belongs to `env_raw`; `out` is written when the call
    // reports success and read only then.
    unsafe {
        if enif_get_map_value(env_raw, map, key, out.as_mut_ptr()) != 0 {
            Some(out.assume_init())
        } else {
            None
        }
    }
}

/// Append `"<iso8601>"` for a calendar struct whose `__struct__` value is
/// `module`. Returns false, with `buf` untouched, for any other struct or
/// one the native path declines.
pub(crate) fn write_iso8601(
    env_raw: *mut ErlNifEnv,
    map: ERL_NIF_TERM,
    module: ERL_NIF_TERM,
    buf: &mut Vec<u8>,
) -> bool {
    let kind = if module == atoms::datetime_module().as_c_arg() {
        Kind::DateTime
    } else if module == atoms::naive_datetime_module().as_c_arg() {
        Kind::NaiveDateTime
    } else if module == atoms::date_module().as_c_arg() {
        Kind::Date
    } else if module == atoms::time_module().as_c_arg() {
        Kind::Time
    } else {
        return false;
    };
    let start = buf.len();
    if write_kind(env_raw, map, kind, buf).is_none() {
        buf.truncate(start);
        return false;
    }
    true
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Date,
    Time,
    NaiveDateTime,
    DateTime,
}

fn write_kind(
    env_raw: *mut ErlNifEnv,
    map: ERL_NIF_TERM,
    kind: Kind,
    buf: &mut Vec<u8>,
) -> Option<()> {
    if map_get(env_raw, map, atoms::calendar().as_c_arg())? != atoms::calendar_iso().as_c_arg() {
        return None;
    }
    buf.push(b'"');
    if kind != Kind::Time {
        write_date(env_raw, map, buf)?;
    }
    if matches!(kind, Kind::NaiveDateTime | Kind::DateTime) {
        buf.push(b'T');
    }
    if kind != Kind::Date {
        write_time(env_raw, map, buf)?;
    }
    if kind == Kind::DateTime {
        write_offset(env_raw, map, buf)?;
    }
    buf.push(b'"');
    Some(())
}

/// `Calendar.ISO.date_to_iodata/4`: the year is any integer, padded to four
/// digits after its sign.
fn write_date(env_raw: *mut ErlNifEnv, map: ERL_NIF_TERM, buf: &mut Vec<u8>) -> Option<()> {
    let year = int_field(env_raw, map, atoms::year().as_c_arg())?;
    let month = ranged(env_raw, map, atoms::month().as_c_arg(), 1, 12)?;
    let day = ranged(env_raw, map, atoms::day().as_c_arg(), 1, 31)?;
    zero_pad(buf, year, 4);
    buf.push(b'-');
    zero_pad(buf, month, 2);
    buf.push(b'-');
    zero_pad(buf, day, 2);
    Some(())
}

/// `Calendar.ISO.time_to_iodata/5`: the fraction is the microsecond value
/// truncated to its precision, and omitted at precision 0.
fn write_time(env_raw: *mut ErlNifEnv, map: ERL_NIF_TERM, buf: &mut Vec<u8>) -> Option<()> {
    let hour = ranged(env_raw, map, atoms::hour().as_c_arg(), 0, 23)?;
    let minute = ranged(env_raw, map, atoms::minute().as_c_arg(), 0, 59)?;
    let second = ranged(env_raw, map, atoms::second().as_c_arg(), 0, 59)?;
    let (value, precision) = microsecond(env_raw, map)?;
    zero_pad(buf, hour, 2);
    buf.push(b':');
    zero_pad(buf, minute, 2);
    buf.push(b':');
    zero_pad(buf, second, 2);
    if precision > 0 {
        buf.push(b'.');
        zero_pad(
            buf,
            value / 10i64.pow(6 - precision as u32),
            precision as usize,
        );
    }
    Some(())
}

/// `Calendar.ISO.offset_to_iodata/4` in extended format.
fn write_offset(env_raw: *mut ErlNifEnv, map: ERL_NIF_TERM, buf: &mut Vec<u8>) -> Option<()> {
    let utc = int_field(env_raw, map, atoms::utc_offset().as_c_arg())?;
    let std = int_field(env_raw, map, atoms::std_offset().as_c_arg())?;
    let zone = binary(
        env_raw,
        map_get(env_raw, map, atoms::time_zone().as_c_arg())?,
    )?;
    if utc == 0 && std == 0 && zone == b"Etc/UTC" {
        buf.push(b'Z');
        return Some(());
    }
    let total = utc.checked_add(std)?;
    buf.push(if total < 0 { b'-' } else { b'+' });
    let second = total.unsigned_abs();
    zero_pad_unsigned(buf, second / 3600, 2);
    buf.push(b':');
    zero_pad_unsigned(buf, second % 3600 / 60, 2);
    Some(())
}

fn microsecond(env_raw: *mut ErlNifEnv, map: ERL_NIF_TERM) -> Option<(i64, i64)> {
    let tuple = map_get(env_raw, map, atoms::microsecond().as_c_arg())?;
    let mut arity = 0;
    let mut elements = MaybeUninit::uninit();
    // SAFETY: on success `elements` points at the tuple's `arity` elements,
    // which live as long as the tuple.
    let elements = unsafe {
        if rustler::sys::enif_get_tuple(env_raw, tuple, &mut arity, elements.as_mut_ptr()) == 0
            || arity != 2
        {
            return None;
        }
        std::slice::from_raw_parts(elements.assume_init(), 2)
    };
    let value = int(env_raw, elements[0]).filter(|v| (0..=999_999).contains(v))?;
    let precision = int(env_raw, elements[1]).filter(|p| (0..=6).contains(p))?;
    Some((value, precision))
}

#[inline]
fn ranged(
    env_raw: *mut ErlNifEnv,
    map: ERL_NIF_TERM,
    key: ERL_NIF_TERM,
    lo: i64,
    hi: i64,
) -> Option<i64> {
    int_field(env_raw, map, key).filter(|v| (lo..=hi).contains(v))
}

#[inline]
fn int_field(env_raw: *mut ErlNifEnv, map: ERL_NIF_TERM, key: ERL_NIF_TERM) -> Option<i64> {
    int(env_raw, map_get(env_raw, map, key)?)
}

#[inline]
fn int(env_raw: *mut ErlNifEnv, term: ERL_NIF_TERM) -> Option<i64> {
    let mut n: i64 = 0;
    // SAFETY: `term` belongs to `env_raw`, and `n` is a local the call writes.
    (unsafe { enif_get_int64(env_raw, term, &mut n) } != 0).then_some(n)
}

fn binary<'a>(env_raw: *mut ErlNifEnv, term: ERL_NIF_TERM) -> Option<&'a [u8]> {
    let mut bin = MaybeUninit::<ErlNifBinary>::uninit();
    // SAFETY: on success `bin` describes bytes that live as long as `term`,
    // which outlives the encode call that reads them.
    unsafe {
        if enif_inspect_binary(env_raw, term, bin.as_mut_ptr()) == 0 {
            return None;
        }
        let bin = bin.assume_init();
        Some(std::slice::from_raw_parts(bin.data, bin.size))
    }
}

/// `Calendar.ISO`'s `zero_pad/2`: the sign, then the magnitude padded to
/// `width` digits and never truncated.
#[inline]
fn zero_pad(buf: &mut Vec<u8>, value: i64, width: usize) {
    if value < 0 {
        buf.push(b'-');
    }
    zero_pad_unsigned(buf, value.unsigned_abs(), width);
}

#[inline]
fn zero_pad_unsigned(buf: &mut Vec<u8>, value: u64, width: usize) {
    let mut digits = itoa::Buffer::new();
    let digits = digits.format(value).as_bytes();
    buf.extend(std::iter::repeat_n(
        b'0',
        width.saturating_sub(digits.len()),
    ));
    buf.extend_from_slice(digits);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn padded(value: i64, width: usize) -> String {
        let mut buf = Vec::new();
        zero_pad(&mut buf, value, width);
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn zero_pad_matches_calendar_iso() {
        assert_eq!(padded(5, 2), "05");
        assert_eq!(padded(2026, 4), "2026");
        assert_eq!(padded(12345, 4), "12345");
        assert_eq!(padded(-99, 4), "-0099");
        assert_eq!(padded(0, 6), "000000");
        assert_eq!(padded(i64::MIN, 4), "-9223372036854775808");
    }
}
