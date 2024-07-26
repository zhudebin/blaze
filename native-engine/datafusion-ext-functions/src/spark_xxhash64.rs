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

use std::sync::Arc;

use arrow::array::*;
use datafusion::{common::Result, physical_plan::ColumnarValue};
use datafusion_ext_commons::spark_hash::create_xxhash64_hashes;

/// implements org.apache.spark.sql.catalyst.expressions.XxHash64
pub fn spark_xxhash64(args: &[ColumnarValue]) -> Result<ColumnarValue> {
    let len = args
        .iter()
        .map(|arg| match arg {
            ColumnarValue::Array(array) => array.len(),
            ColumnarValue::Scalar(_) => 1,
        })
        .max()
        .unwrap_or(0);

    let arrays = args
        .iter()
        .map(|arg| {
            Ok(match arg {
                ColumnarValue::Array(array) => array.clone(),
                ColumnarValue::Scalar(scalar) => scalar.to_array_of_size(len)?,
            })
        })
        .collect::<Result<Vec<_>>>()?;

    // use identical seed as spark hash partition
    let spark_xxhash64_default_seed = 42i64;
    let mut hash_buffer = vec![spark_xxhash64_default_seed; len];
    create_xxhash64_hashes(&arrays, &mut hash_buffer)?;

    Ok(ColumnarValue::Array(Arc::new(
        Int64Array::from_iter_values(hash_buffer.into_iter().map(|hash| hash)),
    )))
}

#[cfg(test)]
mod test {
    use std::{error::Error, sync::Arc};

    use arrow::array::{ArrayRef, Int64Array, StringArray};
    use datafusion::logical_expr::ColumnarValue;

    use super::*;

    #[test]
    fn test_xxhash64_int64() -> Result<(), Box<dyn Error>> {
        let result = spark_xxhash64(&vec![ColumnarValue::Array(Arc::new(Int64Array::from(
            vec![Some(1), Some(0), Some(-1), Some(i64::MAX), Some(i64::MIN)],
        )))])?
        .into_array(5)?;

        let expected = Int64Array::from(vec![
            Some(-7001672635703045582),
            Some(-5252525462095825812),
            Some(3858142552250413010),
            Some(-3246596055638297850),
            Some(-8619748838626508300),
        ]);
        let expected: ArrayRef = Arc::new(expected);

        assert_eq!(&result, &expected);
        Ok(())
    }

    #[test]
    fn test_xxhash64_string() -> Result<(), Box<dyn Error>> {
        let result = spark_xxhash64(&vec![ColumnarValue::Array(Arc::new(
            StringArray::from_iter_values(["hello", "bar", "", "😁", "天地"]),
        ))])?
        .into_array(5)?;

        let expected = Int64Array::from(vec![
            Some(-4367754540140381902),
            Some(-1798770879548125814),
            Some(-7444071767201028348),
            Some(-6337236088984028203),
            Some(-235771157374669727),
        ]);
        let expected: ArrayRef = Arc::new(expected);

        assert_eq!(&result, &expected);
        Ok(())
    }
}
