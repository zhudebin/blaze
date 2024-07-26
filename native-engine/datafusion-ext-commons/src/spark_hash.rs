// Copyright 2022 The Blaze Authors
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Functionality used both on logical and physical plans

use std::sync::Arc;

use arrow::{
    array::*,
    datatypes::{
        ArrowDictionaryKeyType, ArrowNativeType, DataType, Int16Type, Int32Type, Int64Type,
        Int8Type, TimeUnit,
    },
};
use datafusion::error::Result;

use crate::df_execution_err;

#[inline]
pub fn spark_compatible_murmur3_hash<T: AsRef<[u8]>>(data: T, seed: i32) -> i32 {
    let data = data.as_ref();
    let len = data.len();
    let len_aligned = len - len % 4;

    // safety:
    // avoid boundary checking in performance critical codes.
    // all operations are garenteed to be safe
    unsafe {
        let mut h1 =
            mur3::hash_bytes_by_int(std::slice::from_raw_parts(data.as_ptr(), len_aligned), seed);

        for i in len_aligned..len {
            let half_word = *data.get_unchecked(i) as i8 as i32;
            h1 = mur3::mix_h1(h1, mur3::mix_k1(half_word));
        }
        mur3::fmix(h1, len as i32)
    }
}

#[inline]
pub fn spark_compatible_murmur3_hash_long(value: i64, seed: i32) -> i32 {
    mur3::hash_long(value, seed)
}

mod mur3 {
    #[inline]
    pub fn mix_k1(mut k1: i32) -> i32 {
        k1 *= 0xcc9e2d51u32 as i32;
        k1 = k1.rotate_left(15);
        k1 *= 0x1b873593u32 as i32;
        k1
    }

    #[inline]
    pub fn mix_h1(mut h1: i32, k1: i32) -> i32 {
        h1 ^= k1;
        h1 = h1.rotate_left(13);
        h1 = h1 * 5 + 0xe6546b64u32 as i32;
        h1
    }

    #[inline]
    pub fn fmix(mut h1: i32, len: i32) -> i32 {
        h1 ^= len;
        h1 ^= ((h1 as u32) >> 16) as i32;
        h1 *= 0x85ebca6bu32 as i32;
        h1 ^= ((h1 as u32) >> 13) as i32;
        h1 *= 0xc2b2ae35u32 as i32;
        h1 ^= ((h1 as u32) >> 16) as i32;
        h1
    }

    #[inline]
    pub unsafe fn hash_bytes_by_int(data: &[u8], seed: i32) -> i32 {
        // safety: data length must be aligned to 4 bytes
        let mut h1 = seed as i32;
        for i in (0..data.len()).step_by(4) {
            let mut half_word = std::ptr::read_unaligned(data.as_ptr().add(i) as *const i32);
            if cfg!(target_endian = "big") {
                half_word = half_word.reverse_bits();
            }
            h1 = mix_h1(h1, mix_k1(half_word));
        }
        h1
    }

    #[inline]
    pub fn hash_long(input: i64, seed: i32) -> i32 {
        let low = input as i32;
        let high = (input >> 32) as i32;

        let k1 = mix_k1(low);
        let h1 = mix_h1(seed, k1);

        let k1 = mix_k1(high);
        let h1 = mix_h1(h1, k1);

        fmix(h1, 8)
    }
}

#[inline]
pub fn spark_compatible_xxhash64_hash<T: AsRef<[u8]>>(data: T, seed: i64) -> i64 {
    xxhash64::xxhash64(data.as_ref(), seed as u64) as i64
}

mod xxhash64 {
    const PRIME64_1: u64 = 0x9E3779B185EBCA87u64;
    const PRIME64_2: u64 = 0xC2B2AE3D27D4EB4Fu64;
    const PRIME64_3: u64 = 0x165667B19E3779F9u64;
    const PRIME64_4: u64 = 0x85EBCA77C2B2AE63u64;
    const PRIME64_5: u64 = 0x27D4EB2F165667C5u64;

    pub fn xxhash64(input: &[u8], seed: u64) -> u64 {
        let mut hash;
        let mut remaining = input.len();
        let mut offset = 0;

        if remaining >= 32 {
            let mut acc1 = seed + PRIME64_1 + PRIME64_2;
            let mut acc2 = seed + PRIME64_2;
            let mut acc3 = seed + 0;
            let mut acc4 = seed - PRIME64_1;

            while remaining >= 32 {
                acc1 = xxh64_round(acc1, xxh_read64(input, offset));
                offset += 8;
                acc2 = xxh64_round(acc2, xxh_read64(input, offset));
                offset += 8;
                acc3 = xxh64_round(acc3, xxh_read64(input, offset));
                offset += 8;
                acc4 = xxh64_round(acc4, xxh_read64(input, offset));
                offset += 8;
                remaining -= 32;
            }
            hash = xxh_rotl64(acc1, 1)
                + xxh_rotl64(acc2, 7)
                + xxh_rotl64(acc3, 12)
                + xxh_rotl64(acc4, 18);
            hash = xxh64_merge_round(hash, acc1);
            hash = xxh64_merge_round(hash, acc2);
            hash = xxh64_merge_round(hash, acc3);
            hash = xxh64_merge_round(hash, acc4);
        } else {
            hash = seed + PRIME64_5;
        }
        hash += input.len() as u64;

        while remaining >= 8 {
            hash ^= xxh64_round(0, xxh_read64(input, offset));
            hash = xxh_rotl64(hash, 27);
            hash *= PRIME64_1;
            hash += PRIME64_4;
            offset += 8;
            remaining -= 8;
        }

        if remaining >= 4 {
            hash ^= xxh_read32(input, offset) as u64 * PRIME64_1;
            hash = xxh_rotl64(hash, 23);
            hash *= PRIME64_2;
            hash += PRIME64_3;
            offset += 4;
            remaining -= 4;
        }

        while remaining != 0 {
            hash ^= input[offset] as u64 * PRIME64_5;
            hash = xxh_rotl64(hash, 11);
            hash *= PRIME64_1;
            offset += 1;
            remaining -= 1;
        }
        xxh64_avalanche(hash)
    }

    fn xxh64_merge_round(mut hash: u64, acc: u64) -> u64 {
        hash ^= xxh64_round(0, acc);
        hash *= PRIME64_1;
        hash += PRIME64_4;
        hash
    }

    fn xxh_rotl64(value: u64, amt: i32) -> u64 {
        (value << (amt % 64)) | (value >> (64 - amt % 64))
    }

    fn xxh64_round(mut acc: u64, input: u64) -> u64 {
        acc += input * PRIME64_2;
        acc = xxh_rotl64(acc, 31);
        acc *= PRIME64_1;
        acc
    }

    fn xxh64_avalanche(mut hash: u64) -> u64 {
        hash ^= hash >> 33;
        hash *= PRIME64_2;
        hash ^= hash >> 29;
        hash *= PRIME64_3;
        hash ^= hash >> 32;
        return hash;
    }

    fn xxh_read32(data: &[u8], offset: usize) -> u32 {
        unsafe {
            // safety: boundary check is done by caller
            std::ptr::read_unaligned(data.as_ptr().add(offset) as *const u32)
        }
    }

    fn xxh_read64(data: &[u8], offset: usize) -> u64 {
        unsafe {
            // safety: boundary check is done by caller
            std::ptr::read_unaligned(data.as_ptr().add(offset) as *const u64)
        }
    }
}

macro_rules! hash_array {
    ($array_type:ident, $column:ident, $hashes:ident, $h:expr) => {
        let array = $column.as_any().downcast_ref::<$array_type>().unwrap();
        if array.null_count() == 0 {
            for (i, hash) in $hashes.iter_mut().enumerate() {
                *hash = $h(&array.value(i).as_ref(), *hash);
            }
        } else {
            for (i, hash) in $hashes.iter_mut().enumerate() {
                if !array.is_null(i) {
                    *hash = $h(&array.value(i).as_ref(), *hash);
                }
            }
        }
    };
}

macro_rules! hash_array_primitive {
    ($array_type:ident, $column:ident, $ty:ident, $hashes:ident, $h:expr) => {
        let array = $column.as_any().downcast_ref::<$array_type>().unwrap();
        let values = array.values();

        if array.null_count() == 0 {
            for (hash, value) in $hashes.iter_mut().zip(values.iter()) {
                *hash = $h((*value as $ty).to_le_bytes().as_ref(), *hash);
            }
        } else {
            for (i, (hash, value)) in $hashes.iter_mut().zip(values.iter()).enumerate() {
                if !array.is_null(i) {
                    *hash = $h((*value as $ty).to_le_bytes().as_ref(), *hash);
                }
            }
        }
    };
}

macro_rules! hash_array_decimal {
    ($array_type:ident, $column:ident, $hashes:ident, $h:expr) => {
        let array = $column.as_any().downcast_ref::<$array_type>().unwrap();

        if array.null_count() == 0 {
            for (i, hash) in $hashes.iter_mut().enumerate() {
                *hash = $h(array.value(i).to_le_bytes().as_ref(), *hash);
            }
        } else {
            for (i, hash) in $hashes.iter_mut().enumerate() {
                if !array.is_null(i) {
                    *hash = $h(array.value(i).to_le_bytes().as_ref(), *hash);
                }
            }
        }
    };
}

/// Hash the values in a dictionary array
fn create_hashes_dictionary<K: ArrowDictionaryKeyType, T: num::PrimInt>(
    array: &ArrayRef,
    hashes_buffer: &mut [T],
    h: impl Fn(&[u8], T) -> T + Copy,
) -> Result<()> {
    let dict_array = array.as_any().downcast_ref::<DictionaryArray<K>>().unwrap();

    // Hash each dictionary value once, and then use that computed
    // hash for each key value to avoid a potentially expensive
    // redundant hashing for large dictionary elements (e.g. strings)
    let dict_values = Arc::clone(dict_array.values());
    let mut dict_hashes = vec![T::zero(); dict_values.len()];
    create_hashes(&[dict_values], &mut dict_hashes, h)?;

    for (hash, key) in hashes_buffer.iter_mut().zip(dict_array.keys().iter()) {
        if let Some(key) = key {
            if let Some(idx) = key.to_usize() {
                *hash = dict_hashes[idx];
            } else {
                let dt = dict_array.data_type();
                df_execution_err!(
                    "Can not convert key value {key:?} to usize in dictionary of type {dt:?}"
                )?;
            }
        } // no update for Null, consistent with other hashes
    }
    Ok(())
}

pub fn create_murmur3_hashes(arrays: &[ArrayRef], hashes_buffer: &mut [i32]) -> Result<()> {
    create_hashes(arrays, hashes_buffer, |data: &[u8], seed: i32| {
        spark_compatible_murmur3_hash(data, seed)
    })
}

pub fn create_xxhash64_hashes(arrays: &[ArrayRef], hashes_buffer: &mut [i64]) -> Result<()> {
    create_hashes(arrays, hashes_buffer, |data: &[u8], seed: i64| {
        spark_compatible_xxhash64_hash(data, seed)
    })
}

/// Creates hash values for every row, based on the values in the
/// columns.
///
/// The number of rows to hash is determined by `hashes_buffer.len()`.
/// `hashes_buffer` should be pre-sized appropriately
pub fn create_hashes<T: num::PrimInt>(
    arrays: &[ArrayRef],
    hashes_buffer: &mut [T],
    h: impl Fn(&[u8], T) -> T + Copy,
) -> Result<()> {
    for col in arrays {
        hash_array(col, hashes_buffer, h)?;
    }
    Ok(())
}

fn hash_array<T: num::PrimInt>(
    array: &ArrayRef,
    hashes_buffer: &mut [T],
    h: impl Fn(&[u8], T) -> T + Copy,
) -> Result<()> {
    match array.data_type() {
        DataType::Null => {}
        DataType::Boolean => {
            let array = array.as_any().downcast_ref::<BooleanArray>().unwrap();
            if array.null_count() == 0 {
                for (i, hash) in hashes_buffer.iter_mut().enumerate() {
                    *hash = h(
                        (if array.value(i) { 1u32 } else { 0u32 })
                            .to_le_bytes()
                            .as_ref(),
                        *hash,
                    );
                }
            } else {
                for (i, hash) in hashes_buffer.iter_mut().enumerate() {
                    if !array.is_null(i) {
                        *hash = h(
                            (if array.value(i) { 1u32 } else { 0u32 })
                                .to_le_bytes()
                                .as_ref(),
                            *hash,
                        );
                    }
                }
            }
        }
        DataType::Int8 => {
            hash_array_primitive!(Int8Array, array, i32, hashes_buffer, h);
        }
        DataType::Int16 => {
            hash_array_primitive!(Int16Array, array, i32, hashes_buffer, h);
        }
        DataType::Int32 => {
            hash_array_primitive!(Int32Array, array, i32, hashes_buffer, h);
        }
        DataType::Int64 => {
            hash_array_primitive!(Int64Array, array, i64, hashes_buffer, h);
        }
        DataType::Float32 => {
            hash_array_primitive!(Float32Array, array, f32, hashes_buffer, h);
        }
        DataType::Float64 => {
            hash_array_primitive!(Float64Array, array, f64, hashes_buffer, h);
        }
        DataType::Timestamp(TimeUnit::Second, _) => {
            hash_array_primitive!(TimestampSecondArray, array, i64, hashes_buffer, h);
        }
        DataType::Timestamp(TimeUnit::Millisecond, _) => {
            hash_array_primitive!(TimestampMillisecondArray, array, i64, hashes_buffer, h);
        }
        DataType::Timestamp(TimeUnit::Microsecond, _) => {
            hash_array_primitive!(TimestampMicrosecondArray, array, i64, hashes_buffer, h);
        }
        DataType::Timestamp(TimeUnit::Nanosecond, _) => {
            hash_array_primitive!(TimestampNanosecondArray, array, i64, hashes_buffer, h);
        }
        DataType::Date32 => {
            hash_array_primitive!(Date32Array, array, i32, hashes_buffer, h);
        }
        DataType::Date64 => {
            hash_array_primitive!(Date64Array, array, i64, hashes_buffer, h);
        }
        DataType::Binary => {
            hash_array!(BinaryArray, array, hashes_buffer, h);
        }
        DataType::LargeBinary => {
            hash_array!(LargeBinaryArray, array, hashes_buffer, h);
        }
        DataType::Utf8 => {
            hash_array!(StringArray, array, hashes_buffer, h);
        }
        DataType::LargeUtf8 => {
            hash_array!(LargeStringArray, array, hashes_buffer, h);
        }
        DataType::Decimal128(..) => {
            hash_array_decimal!(Decimal128Array, array, hashes_buffer, h);
        }
        DataType::Dictionary(index_type, _) => match &**index_type {
            DataType::Int8 => create_hashes_dictionary::<Int8Type, _>(array, hashes_buffer, h)?,
            DataType::Int16 => create_hashes_dictionary::<Int16Type, _>(array, hashes_buffer, h)?,
            DataType::Int32 => create_hashes_dictionary::<Int32Type, _>(array, hashes_buffer, h)?,
            DataType::Int64 => create_hashes_dictionary::<Int64Type, _>(array, hashes_buffer, h)?,
            other => df_execution_err!("Unsupported dictionary type in hasher hashing: {other}")?,
        },
        _ => {
            for idx in 0..array.len() {
                hash_one(array, idx, &mut hashes_buffer[idx], h)?;
            }
        }
    }
    Ok(())
}

macro_rules! hash_one_primitive {
    ($array_type:ident, $column:ident, $ty:ident, $hash:ident, $idx:ident, $h:expr) => {
        let array = $column.as_any().downcast_ref::<$array_type>().unwrap();
        *$hash = $h(
            (array.value($idx as usize) as $ty).to_le_bytes().as_ref(),
            *$hash,
        );
    };
}

macro_rules! hash_one_binary {
    ($array_type:ident, $column:ident, $hash:ident, $idx:ident, $h:expr) => {
        let array = $column.as_any().downcast_ref::<$array_type>().unwrap();
        *$hash = $h(&array.value($idx as usize).as_ref(), *$hash);
    };
}

macro_rules! hash_one_decimal {
    ($array_type:ident, $column:ident, $hash:ident, $idx:ident, $h:expr) => {
        let array = $column.as_any().downcast_ref::<$array_type>().unwrap();
        *$hash = $h(array.value($idx as usize).to_le_bytes().as_ref(), *$hash);
    };
}

fn hash_one<T: num::PrimInt>(
    col: &ArrayRef,
    idx: usize,
    hash: &mut T,
    h: impl Fn(&[u8], T) -> T + Copy,
) -> Result<()> {
    if col.is_valid(idx) {
        match col.data_type() {
            DataType::Null => {}
            DataType::Boolean => {
                let array = col.as_any().downcast_ref::<BooleanArray>().unwrap();
                *hash = h(
                    (if array.value(idx) { 1u32 } else { 0u32 })
                        .to_le_bytes()
                        .as_ref(),
                    *hash,
                );
            }
            DataType::Int8 => {
                hash_one_primitive!(Int8Array, col, i32, hash, idx, h);
            }
            DataType::Int16 => {
                hash_one_primitive!(Int16Array, col, i32, hash, idx, h);
            }
            DataType::Int32 => {
                hash_one_primitive!(Int32Array, col, i32, hash, idx, h);
            }
            DataType::Int64 => {
                hash_one_primitive!(Int64Array, col, i64, hash, idx, h);
            }
            DataType::Float32 => {
                hash_one_primitive!(Float32Array, col, f32, hash, idx, h);
            }
            DataType::Float64 => {
                hash_one_primitive!(Float64Array, col, f64, hash, idx, h);
            }
            DataType::Timestamp(TimeUnit::Second, None) => {
                hash_one_primitive!(TimestampSecondArray, col, i64, hash, idx, h);
            }
            DataType::Timestamp(TimeUnit::Millisecond, None) => {
                hash_one_primitive!(TimestampMillisecondArray, col, i64, hash, idx, h);
            }
            DataType::Timestamp(TimeUnit::Microsecond, None) => {
                hash_one_primitive!(TimestampMicrosecondArray, col, i64, hash, idx, h);
            }
            DataType::Timestamp(TimeUnit::Nanosecond, _) => {
                hash_one_primitive!(TimestampNanosecondArray, col, i64, hash, idx, h);
            }
            DataType::Date32 => {
                hash_one_primitive!(Date32Array, col, i32, hash, idx, h);
            }
            DataType::Date64 => {
                hash_one_primitive!(Date64Array, col, i64, hash, idx, h);
            }
            DataType::Binary => {
                hash_one_binary!(BinaryArray, col, hash, idx, h);
            }
            DataType::LargeBinary => {
                hash_one_binary!(LargeBinaryArray, col, hash, idx, h);
            }
            DataType::Utf8 => {
                hash_one_binary!(StringArray, col, hash, idx, h);
            }
            DataType::LargeUtf8 => {
                hash_one_binary!(LargeStringArray, col, hash, idx, h);
            }
            DataType::Decimal128(..) => {
                hash_one_decimal!(Decimal128Array, col, hash, idx, h);
            }
            DataType::List(..) => {
                let list_array = col.as_any().downcast_ref::<ListArray>().unwrap();
                let value_array = list_array.value(idx);
                for i in 0..value_array.len() {
                    hash_one(&value_array, i, hash, h)?;
                }
            }
            DataType::Map(..) => {
                let map_array = col.as_any().downcast_ref::<MapArray>().unwrap();
                let kv_array = map_array.value(idx);
                let key_array = kv_array.column(0);
                let value_array = kv_array.column(1);
                for i in 0..kv_array.len() {
                    hash_one(key_array, i, hash, h)?;
                    hash_one(value_array, i, hash, h)?;
                }
            }
            DataType::Struct(_) => {
                let struct_array = col.as_any().downcast_ref::<StructArray>().unwrap();
                for col in struct_array.columns() {
                    hash_one(col, idx, hash, h)?;
                }
            }
            other => df_execution_err!("Unsupported data type in hasher: {other}")?,
        }
    }
    Ok(())
}

pub fn pmod(hash: i32, n: usize) -> usize {
    let n = n as i32;
    let r = hash % n;
    let result = if r < 0 { (r + n) % n } else { r };
    result as usize
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use arrow::{
        array::{
            make_array, Array, ArrayData, ArrayRef, Int32Array, Int64Array, Int8Array, MapArray,
            StringArray, StructArray, UInt32Array,
        },
        buffer::Buffer,
        datatypes::{DataType, Field, ToByteSlice},
    };

    use super::*;

    #[test]
    fn test_murmur3() {
        let _hashes = ["", "a", "ab", "abc", "abcd", "abcde"]
            .into_iter()
            .map(|s| spark_compatible_murmur3_hash(s.as_bytes(), 42) as i32)
            .collect::<Vec<_>>();
        let _expected = vec![
            142593372, 1485273170, -97053317, 1322437556, -396302900, 814637928,
        ];
        assert_eq!(_hashes, _expected)
    }

    #[test]
    fn test_xxhash64() {
        let _hashes = [
            "",
            "a",
            "ab",
            "abc",
            "abcd",
            "abcde",
            "abcdefghijklmnopqrstuvwxyz",
        ]
        .into_iter()
        .map(|s| spark_compatible_xxhash64_hash(s.as_bytes(), 42))
        .collect::<Vec<_>>();
        let _expected = vec![
            -7444071767201028348,
            -8582455328737087284,
            2710560539726725091,
            1423657621850124518,
            -6810745876291105281,
            -990457398947679591,
            -3265757659154784300,
        ];
        assert_eq!(_hashes, _expected)
    }

    #[test]
    fn test_list() {
        let mut hashes_buffer = vec![42; 4];
        for hash in hashes_buffer.iter_mut() {
            *hash = spark_compatible_murmur3_hash(5_i32.to_le_bytes(), *hash);
        }
    }

    #[test]
    fn test_i8() {
        let i = Arc::new(Int8Array::from(vec![
            Some(1),
            Some(0),
            Some(-1),
            Some(i8::MAX),
            Some(i8::MIN),
        ])) as ArrayRef;
        let mut hashes = vec![42; 5];
        create_murmur3_hashes(&[i], &mut hashes).unwrap();

        // generated with Spark Murmur3_x86_32
        let expected: Vec<i32> = [
            0xdea578e3_u32,
            0x379fae8f,
            0xa0590e3d,
            0x43b4d8ed,
            0x422a1365,
        ]
        .into_iter()
        .map(|v| v as i32)
        .collect();
        assert_eq!(hashes, expected);
    }

    #[test]
    fn test_i32() {
        let i = Arc::new(Int32Array::from(vec![Some(1)])) as ArrayRef;
        let mut hashes = vec![42; 1];
        create_murmur3_hashes(&[i], &mut hashes).unwrap();

        let j = Arc::new(Int32Array::from(vec![Some(2)])) as ArrayRef;
        create_murmur3_hashes(&[j], &mut hashes).unwrap();

        let m = Arc::new(Int32Array::from(vec![Some(3)])) as ArrayRef;
        create_murmur3_hashes(&[m], &mut hashes).unwrap();

        let n = Arc::new(Int32Array::from(vec![Some(4)])) as ArrayRef;
        create_murmur3_hashes(&[n], &mut hashes).unwrap();
    }

    #[test]
    fn test_i64() {
        let i = Arc::new(Int64Array::from(vec![
            Some(1),
            Some(0),
            Some(-1),
            Some(i64::MAX),
            Some(i64::MIN),
        ])) as ArrayRef;

        // generated with Murmur3Hash(Seq(Literal(1L)), 42).eval() since Spark is tested
        let mut hashes = vec![42; 5];
        create_murmur3_hashes(&[i.clone()], &mut hashes).unwrap();
        let expected: Vec<i32> = [
            0x99f0149d_u32,
            0x9c67b85d,
            0xc8008529,
            0xa05b5d7b,
            0xcd1e64fb,
        ]
        .into_iter()
        .map(|v| v as i32)
        .collect();
        assert_eq!(hashes, expected);

        // generated with XxHash64(Seq(Literal(1L)), 42).eval() since Spark is tested
        // against this as well
        let mut hashes = vec![42; 5];
        create_xxhash64_hashes(&[i.clone()], &mut hashes).unwrap();
        let expected = vec![
            -7001672635703045582,
            -5252525462095825812,
            3858142552250413010,
            -3246596055638297850,
            -8619748838626508300,
        ];
        assert_eq!(hashes, expected);
    }

    #[test]
    fn test_str() {
        let i = Arc::new(StringArray::from(vec!["hello", "bar", "", "😁", "天地"]));

        // generated with Murmur3Hash(Seq(Literal("")), 42).eval() since Spark is tested
        // against this as well
        let mut hashes = vec![42; 5];
        create_murmur3_hashes(&[i.clone()], &mut hashes).unwrap();
        let expected: Vec<i32> = [3286402344_u32, 2486176763, 142593372, 885025535, 2395000894]
            .into_iter()
            .map(|v| v as i32)
            .collect();
        assert_eq!(hashes, expected);

        // generated with XxHash64(Seq(Literal("")), 42).eval() since Spark is tested
        // against this as well
        let mut hashes = vec![42; 5];
        create_xxhash64_hashes(&[i.clone()], &mut hashes).unwrap();
        let expected = vec![
            -4367754540140381902,
            -1798770879548125814,
            -7444071767201028348,
            -6337236088984028203,
            -235771157374669727,
        ];
        assert_eq!(hashes, expected);
    }

    #[test]
    fn test_pmod() {
        let i: Vec<i32> = [
            0x99f0149d_u32,
            0x9c67b85d,
            0xc8008529,
            0xa05b5d7b,
            0xcd1e64fb,
        ]
        .into_iter()
        .map(|v| v as i32)
        .collect();
        let result = i.into_iter().map(|i| pmod(i, 200)).collect::<Vec<usize>>();

        // expected partition from Spark with n=200
        let expected = vec![69, 5, 193, 171, 115];
        assert_eq!(result, expected);
    }

    #[test]
    fn test_map_array() {
        // Construct key and values
        let key_data = ArrayData::builder(DataType::Int32)
            .len(8)
            .add_buffer(Buffer::from(&[0, 1, 2, 3, 4, 5, 6, 7].to_byte_slice()))
            .build()
            .unwrap();
        let value_data = ArrayData::builder(DataType::UInt32)
            .len(8)
            .add_buffer(Buffer::from(
                &[0u32, 10, 20, 0, 40, 0, 60, 70].to_byte_slice(),
            ))
            .null_bit_buffer(Some(Buffer::from(&[0b11010110])))
            .build()
            .unwrap();

        // Construct a buffer for value offsets, for the nested array:
        //  [[0, 1, 2], [3, 4, 5], [6, 7]]
        let entry_offsets = Buffer::from(&[0, 3, 6, 8].to_byte_slice());

        let keys_field = Arc::new(Field::new("keys", DataType::Int32, false));
        let values_field = Arc::new(Field::new("values", DataType::UInt32, true));
        let entry_struct = StructArray::from(vec![
            (keys_field.clone(), make_array(key_data)),
            (values_field.clone(), make_array(value_data.clone())),
        ]);

        // Construct a map array from the above two
        let map_data_type = DataType::Map(
            Arc::new(Field::new(
                "entries",
                entry_struct.data_type().clone(),
                true,
            )),
            false,
        );
        let map_data = ArrayData::builder(map_data_type)
            .len(3)
            .add_buffer(entry_offsets)
            .add_child_data(entry_struct.into_data())
            .build()
            .unwrap();
        let map_array = MapArray::from(map_data);

        assert_eq!(&value_data, &map_array.values().to_data());
        assert_eq!(&DataType::UInt32, map_array.value_type());
        assert_eq!(3, map_array.len());
        assert_eq!(0, map_array.null_count());
        assert_eq!(6, map_array.value_offsets()[2]);
        assert_eq!(2, map_array.value_length(2));

        let key_array = Arc::new(Int32Array::from(vec![0, 1, 2])) as ArrayRef;
        let value_array =
            Arc::new(UInt32Array::from(vec![None, Some(10u32), Some(20)])) as ArrayRef;
        let struct_array = StructArray::from(vec![
            (keys_field.clone(), key_array),
            (values_field.clone(), value_array),
        ]);
        assert_eq!(
            struct_array,
            StructArray::from(map_array.value(0).into_data())
        );
        assert_eq!(
            &struct_array,
            unsafe { map_array.value_unchecked(0) }
                .as_any()
                .downcast_ref::<StructArray>()
                .unwrap()
        );
        for i in 0..3 {
            assert!(map_array.is_valid(i));
            assert!(!map_array.is_null(i));
        }

        // Now test with a non-zero offset
        let map_data = ArrayData::builder(map_array.data_type().clone())
            .len(2)
            .offset(1)
            .add_buffer(map_array.to_data().buffers()[0].clone())
            .add_child_data(map_array.to_data().child_data()[0].clone())
            .build()
            .unwrap();
        let map_array = MapArray::from(map_data);

        assert_eq!(&value_data, &map_array.values().to_data());
        assert_eq!(&DataType::UInt32, map_array.value_type());
        assert_eq!(2, map_array.len());
        assert_eq!(0, map_array.null_count());
        assert_eq!(6, map_array.value_offsets()[1]);
        assert_eq!(2, map_array.value_length(1));

        let key_array = Arc::new(Int32Array::from(vec![3, 4, 5])) as ArrayRef;
        let value_array = Arc::new(UInt32Array::from(vec![None, Some(40), None])) as ArrayRef;
        let struct_array =
            StructArray::from(vec![(keys_field, key_array), (values_field, value_array)]);
        assert_eq!(
            &struct_array,
            map_array
                .value(0)
                .as_any()
                .downcast_ref::<StructArray>()
                .unwrap()
        );
        assert_eq!(
            &struct_array,
            unsafe { map_array.value_unchecked(0) }
                .as_any()
                .downcast_ref::<StructArray>()
                .unwrap()
        );
    }
}
