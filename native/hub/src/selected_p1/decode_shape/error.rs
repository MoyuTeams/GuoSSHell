use super::{CostInputs, NumericReject, add, mul};

/// 来自实际 fmt 调用和 expected/OneOf 字段表的长度；不预填领域模板或错误常数。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ErrorFormatInputs {
    pub literal_bytes: usize,
    pub expected_bytes: usize,
    pub display_strings: usize,
    pub debug_strings: usize,
    pub decimal_counts: usize,
    /// 若模板包含位置或其他数值，由调用者给其上界；同一输入的 SliceRead line/column
    /// 使用 Shape::position_value_upper() 的 checked 结果，并与其他数值上界取最大。
    pub decimal_value_upper: usize,
    pub other_dynamic_bytes: usize,
    pub has_arguments: bool,
    pub starts_with_argument: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ErrorFormatUpper {
    pub output_bytes: usize,
    pub initial_capacity: usize,
}

impl ErrorFormatInputs {
    /// 锁定 std alloc::fmt 的 estimated_capacity，加逐参数长度界。
    /// 父须另算源 format/message 各自 VPeak、String→Box<str> 旧新和两个 ErrorImpl。
    pub fn upper(self, costs: CostInputs) -> Result<ErrorFormatUpper, NumericReject> {
        let mut digits = 1;
        let mut count = costs.error_count_upper().max(self.decimal_value_upper);
        while count >= 10 {
            count /= 10;
            digits = add(digits, 1)?;
        }
        let mut output_bytes = add(self.literal_bytes, self.expected_bytes)?;
        output_bytes = add(output_bytes, self.other_dynamic_bytes)?;
        output_bytes = add(
            output_bytes,
            mul(self.display_strings, costs.max_decoded_string_bytes_upper)?,
        )?;
        if self.debug_strings != 0 {
            output_bytes = add(
                output_bytes,
                mul(self.debug_strings, costs.debug_string_bytes_upper()?)?,
            )?;
        }
        output_bytes = add(output_bytes, mul(self.decimal_counts, digits)?)?;
        let initial_capacity = if !self.has_arguments {
            self.literal_bytes
        } else if self.starts_with_argument && self.literal_bytes < 16 {
            0
        } else {
            mul(self.literal_bytes, 2)?
        };
        Ok(ErrorFormatUpper {
            output_bytes,
            initial_capacity,
        })
    }
}
