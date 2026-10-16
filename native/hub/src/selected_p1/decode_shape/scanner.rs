use super::{
    ContainerKind, ContainerPrefix, CostInputs, NumericReject, PrefixError, PrefixErrorKind,
    ScratchFrame, ScratchRequirement, Shape, ShapeStatus, add,
};

const ARRAY_FIRST: u8 = 0;
const ARRAY_NEXT: u8 = 1;
const ARRAY_END: u8 = 2;
const OBJECT_FIRST: u8 = 3;
const OBJECT_NEXT: u8 = 4;
const OBJECT_COLON: u8 = 5;
const OBJECT_VALUE: u8 = 6;
const OBJECT_END: u8 = 7;

/// 无堆预扫。合法 scanner 前缀内，每个 opening 增一、closing 减一；
/// 错配 closing、坏 escape/control 在 scanner 中立即终止，预扫其后只会放宽界。
/// 引号内仅跳过 backslash 的后一 byte；坏 \u 中的引号只出现在 scanner 停止之后。
/// 因此该词法最大深度覆盖真实活动 frame，且不超过输入 len。Layout 先 checked。
pub fn needed_scratch(input: &[u8]) -> Result<ScratchRequirement, NumericReject> {
    let mut quoted = false;
    let mut escaped = false;
    let mut depth = 0usize;
    let mut maximum = 0;
    for &byte in input {
        if quoted {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                quoted = false;
            }
        } else {
            match byte {
                b'"' => quoted = true,
                b'[' | b'{' => {
                    depth = add(depth, 1)?;
                    maximum = maximum.max(depth);
                }
                b']' | b'}' => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
    }
    ScratchRequirement::for_slots(maximum)
}

pub fn scan(input: &[u8], scratch: &mut [ScratchFrame]) -> Result<Shape, NumericReject> {
    scan_with(input, scratch, |_| Ok(()))
}

/// callback 只收标量 span/count；其自身若要分配，必须由调用者另行预约。
/// scanner 每个 opening 恰发一次记录；失败时也发尚未闭合的容器前缀。
pub fn scan_with(
    input: &[u8],
    scratch: &mut [ScratchFrame],
    mut container: impl FnMut(ContainerPrefix) -> Result<(), NumericReject>,
) -> Result<Shape, NumericReject> {
    let requirement = needed_scratch(input)?;
    if scratch.len() < requirement.slots {
        return Err(NumericReject::NeededScratch(requirement));
    }
    let mut scanner = Scanner {
        input,
        scratch,
        offset: 0,
        depth: 0,
        array_depth: 0,
        object_depth: 0,
        root_started: false,
        costs: CostInputs::default(),
    };
    let status = loop {
        scanner.whitespace()?;
        let state = scanner.top().map(|frame| frame.state);
        let result = match state {
            None if scanner.root_started => {
                break if scanner.peek().is_none() {
                    ShapeStatus::Complete
                } else {
                    ShapeStatus::Invalid(scanner.error(PrefixErrorKind::TrailingCharacters))
                };
            }
            None => scanner.value(),
            Some(ARRAY_FIRST | ARRAY_NEXT) => {
                if scanner.peek() == Some(b']') {
                    if state == Some(ARRAY_NEXT) {
                        Err(Failure::Syntax(PrefixErrorKind::TrailingComma))
                    } else {
                        scanner.close(&mut container)
                    }
                } else {
                    scanner.value()
                }
            }
            Some(OBJECT_FIRST | OBJECT_NEXT) => {
                if scanner.peek() == Some(b'}') {
                    if state == Some(OBJECT_NEXT) {
                        Err(Failure::Syntax(PrefixErrorKind::TrailingComma))
                    } else {
                        scanner.close(&mut container)
                    }
                } else if scanner.peek() == Some(b'"') {
                    scanner.key()
                } else {
                    Err(Failure::Syntax(if scanner.peek().is_none() {
                        PrefixErrorKind::UnexpectedEnd
                    } else {
                        PrefixErrorKind::ExpectedKey
                    }))
                }
            }
            Some(OBJECT_COLON) => {
                if scanner.peek() == Some(b':') {
                    scanner.advance()?;
                    scanner.top_mut()?.state = OBJECT_VALUE;
                    Ok(())
                } else {
                    Err(Failure::Syntax(if scanner.peek().is_none() {
                        PrefixErrorKind::UnexpectedEnd
                    } else {
                        PrefixErrorKind::ExpectedColon
                    }))
                }
            }
            Some(OBJECT_VALUE) => scanner.value(),
            Some(ARRAY_END | OBJECT_END) => {
                let closing = if state == Some(ARRAY_END) { b']' } else { b'}' };
                match scanner.peek() {
                    Some(byte) if byte == closing => scanner.close(&mut container),
                    Some(b',') => {
                        scanner.advance()?;
                        scanner.top_mut()?.state = if state == Some(ARRAY_END) {
                            ARRAY_NEXT
                        } else {
                            OBJECT_NEXT
                        };
                        Ok(())
                    }
                    other => Err(Failure::Syntax(if other.is_none() {
                        PrefixErrorKind::UnexpectedEnd
                    } else {
                        PrefixErrorKind::ExpectedCommaOrEnd
                    })),
                }
            }
            Some(_) => return Err(NumericReject::ArithmeticOverflow),
        };
        match result {
            Ok(()) => {}
            Err(Failure::Numeric(error)) => return Err(error),
            Err(Failure::Syntax(kind)) => break ShapeStatus::Invalid(scanner.error(kind)),
        }
    };
    while scanner.depth != 0 {
        scanner.finish_container(false, &mut container)?;
    }
    Ok(Shape {
        status,
        prefix_bytes: scanner.offset,
        input_bytes: input.len(),
        costs: scanner.costs,
    })
}

enum Failure {
    Syntax(PrefixErrorKind),
    Numeric(NumericReject),
}

impl From<NumericReject> for Failure {
    fn from(value: NumericReject) -> Self {
        Self::Numeric(value)
    }
}

struct Scanner<'a> {
    input: &'a [u8],
    scratch: &'a mut [ScratchFrame],
    offset: usize,
    depth: usize,
    array_depth: usize,
    object_depth: usize,
    root_started: bool,
    costs: CostInputs,
}

impl Scanner<'_> {
    fn peek(&self) -> Option<u8> {
        self.input.get(self.offset).copied()
    }

    fn advance(&mut self) -> Result<(), NumericReject> {
        self.offset = add(self.offset, 1)?;
        Ok(())
    }

    fn whitespace(&mut self) -> Result<(), NumericReject> {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.advance()?;
        }
        Ok(())
    }

    fn top(&self) -> Option<&ScratchFrame> {
        self.depth.checked_sub(1).and_then(|i| self.scratch.get(i))
    }

    fn top_mut(&mut self) -> Result<&mut ScratchFrame, NumericReject> {
        self.depth
            .checked_sub(1)
            .and_then(|i| self.scratch.get_mut(i))
            .ok_or(NumericReject::ArithmeticOverflow)
    }

    fn error(&self, kind: PrefixErrorKind) -> PrefixError {
        PrefixError {
            kind,
            offset: self.offset,
            active_depth: self.depth,
        }
    }

    fn key(&mut self) -> Result<(), Failure> {
        let frame = self.top_mut()?;
        frame.started = add(frame.started, 1)?;
        frame.state = OBJECT_COLON;
        self.costs.keys_started = add(self.costs.keys_started, 1)?;
        self.string()
    }

    fn value(&mut self) -> Result<(), Failure> {
        let byte = self
            .peek()
            .ok_or(Failure::Syntax(PrefixErrorKind::UnexpectedEnd))?;
        if !matches!(
            byte,
            b'[' | b'{' | b'"' | b'n' | b't' | b'f' | b'-' | b'0'..=b'9'
        ) {
            return Err(Failure::Syntax(PrefixErrorKind::ExpectedValue));
        }
        if let Some(frame) = self.top() {
            if frame.state == ARRAY_FIRST || frame.state == ARRAY_NEXT {
                let frame = self.top_mut()?;
                frame.started = add(frame.started, 1)?;
            }
        } else {
            self.root_started = true;
        }
        match byte {
            b'[' | b'{' => {
                self.costs.tokens_started = add(self.costs.tokens_started, 1)?;
                let frame = self
                    .scratch
                    .get_mut(self.depth)
                    .ok_or(NumericReject::ArithmeticOverflow)?;
                *frame = ScratchFrame {
                    start: self.offset,
                    started: 0,
                    completed: 0,
                    state: if byte == b'[' {
                        ARRAY_FIRST
                    } else {
                        OBJECT_FIRST
                    },
                };
                self.depth = add(self.depth, 1)?;
                if byte == b'[' {
                    self.array_depth = add(self.array_depth, 1)?;
                } else {
                    self.object_depth = add(self.object_depth, 1)?;
                }
                self.costs.max_depth = self.costs.max_depth.max(self.depth);
                self.costs.max_array_depth = self.costs.max_array_depth.max(self.array_depth);
                self.costs.max_object_depth = self.costs.max_object_depth.max(self.object_depth);
                self.costs.ignored_stack_required_upper = self.costs.max_depth;
                self.advance()?;
                Ok(())
            }
            b'"' => {
                self.string()?;
                self.complete_value()?;
                Ok(())
            }
            _ => {
                self.costs.tokens_started = add(self.costs.tokens_started, 1)?;
                match byte {
                    b'n' => self.literal(b"null")?,
                    b't' => self.literal(b"true")?,
                    b'f' => self.literal(b"false")?,
                    _ => self.number()?,
                }
                self.complete_value()?;
                Ok(())
            }
        }
    }

    fn complete_value(&mut self) -> Result<(), NumericReject> {
        if self.depth != 0 {
            let frame = self.top_mut()?;
            frame.completed = add(frame.completed, 1)?;
            frame.state = if frame.state <= ARRAY_END {
                ARRAY_END
            } else {
                OBJECT_END
            };
        }
        Ok(())
    }

    fn close(
        &mut self,
        callback: &mut impl FnMut(ContainerPrefix) -> Result<(), NumericReject>,
    ) -> Result<(), Failure> {
        self.advance()?;
        self.finish_container(true, callback)?;
        self.complete_value()?;
        Ok(())
    }

    fn finish_container(
        &mut self,
        closed: bool,
        callback: &mut impl FnMut(ContainerPrefix) -> Result<(), NumericReject>,
    ) -> Result<(), NumericReject> {
        let frame = *self.top_mut()?;
        let kind = if frame.state <= ARRAY_END {
            ContainerKind::Array
        } else {
            ContainerKind::Object
        };
        let prefix = ContainerPrefix {
            kind,
            start: frame.start,
            end: self.offset,
            started_members: frame.started,
            completed_members: frame.completed,
            closed,
        };
        match kind {
            ContainerKind::Array => {
                self.costs.arrays.include(prefix)?;
                self.array_depth = self
                    .array_depth
                    .checked_sub(1)
                    .ok_or(NumericReject::ArithmeticOverflow)?;
            }
            ContainerKind::Object => {
                self.costs.objects.include(prefix)?;
                self.object_depth = self
                    .object_depth
                    .checked_sub(1)
                    .ok_or(NumericReject::ArithmeticOverflow)?;
            }
        }
        self.depth = self
            .depth
            .checked_sub(1)
            .ok_or(NumericReject::ArithmeticOverflow)?;
        callback(prefix)
    }

    fn literal(&mut self, literal: &[u8]) -> Result<(), Failure> {
        for &expected in literal {
            let actual = self
                .peek()
                .ok_or(Failure::Syntax(PrefixErrorKind::UnexpectedEnd))?;
            self.advance()?;
            if actual != expected {
                return Err(Failure::Syntax(PrefixErrorKind::InvalidLiteral));
            }
        }
        Ok(())
    }

    fn number(&mut self) -> Result<(), Failure> {
        let start = self.offset;
        self.costs.numbers_started = add(self.costs.numbers_started, 1)?;
        let result = self.number_grammar();
        let length = self
            .offset
            .checked_sub(start)
            .ok_or(NumericReject::ArithmeticOverflow)?;
        self.costs.max_number_bytes = self.costs.max_number_bytes.max(length);
        result
    }

    // 只核验 ignore_integer 同形的词法；u8/u16/usize/f32/Value 的范围交旧 codec。
    fn number_grammar(&mut self) -> Result<(), Failure> {
        if self.peek() == Some(b'-') {
            self.advance()?;
        }
        match self.peek() {
            Some(b'0') => {
                self.advance()?;
                if matches!(self.peek(), Some(b'0'..=b'9')) {
                    return Err(Failure::Syntax(PrefixErrorKind::InvalidNumber));
                }
            }
            Some(b'1'..=b'9') => self.digits()?,
            other => {
                return Err(Failure::Syntax(if other.is_none() {
                    PrefixErrorKind::UnexpectedEnd
                } else {
                    PrefixErrorKind::InvalidNumber
                }));
            }
        }
        if self.peek() == Some(b'.') {
            self.advance()?;
            self.required_digits()?;
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.advance()?;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.advance()?;
            }
            self.required_digits()?;
        }
        Ok(())
    }

    fn required_digits(&mut self) -> Result<(), Failure> {
        if !matches!(self.peek(), Some(b'0'..=b'9')) {
            return Err(Failure::Syntax(if self.peek().is_none() {
                PrefixErrorKind::UnexpectedEnd
            } else {
                PrefixErrorKind::InvalidNumber
            }));
        }
        self.digits()?;
        Ok(())
    }

    fn digits(&mut self) -> Result<(), NumericReject> {
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.advance()?;
        }
        Ok(())
    }

    fn string(&mut self) -> Result<(), Failure> {
        self.costs.tokens_started = add(self.costs.tokens_started, 1)?;
        self.costs.strings_started = add(self.costs.strings_started, 1)?;
        self.advance()?;
        let mut metrics = StringMetrics::default();
        let result = self.string_body(&mut metrics);
        self.costs.decoded_string_bytes_upper =
            add(self.costs.decoded_string_bytes_upper, metrics.bytes)?;
        self.costs.max_decoded_string_bytes_upper =
            self.costs.max_decoded_string_bytes_upper.max(metrics.bytes);
        self.costs.max_string_scalars_upper =
            self.costs.max_string_scalars_upper.max(metrics.scalars);
        if metrics.escaped {
            self.costs.escaped_strings = add(self.costs.escaped_strings, 1)?;
            self.costs.max_string_scratch_required_upper = self
                .costs
                .max_string_scratch_required_upper
                .max(metrics.required.max(metrics.bytes));
        }
        if metrics.invalid_utf8 {
            self.costs.strings_with_invalid_utf8 = add(self.costs.strings_with_invalid_utf8, 1)?;
        }
        result
    }

    fn string_body(&mut self, metrics: &mut StringMetrics) -> Result<(), Failure> {
        loop {
            let start = self.offset;
            while matches!(self.peek(), Some(byte) if byte >= 0x20 && byte != b'"' && byte != b'\\')
            {
                self.advance()?;
            }
            let run = self
                .input
                .get(start..self.offset)
                .ok_or(NumericReject::ArithmeticOverflow)?;
            let scalars = match core::str::from_utf8(run) {
                Ok(text) => {
                    let mut count = 0;
                    for _ in text.chars() {
                        count = add(count, 1)?;
                    }
                    count
                }
                Err(_) => {
                    metrics.invalid_utf8 = true;
                    run.len()
                }
            };
            metrics.append(run.len(), scalars, run.len())?;
            match self.peek() {
                Some(b'"') => {
                    self.advance()?;
                    return Ok(());
                }
                Some(b'\\') => {
                    metrics.escaped = true;
                    self.advance()?;
                    self.escape(metrics)?;
                }
                None => return Err(Failure::Syntax(PrefixErrorKind::UnexpectedEnd)),
                _ => return Err(Failure::Syntax(PrefixErrorKind::ControlInString)),
            }
        }
    }

    fn escape(&mut self, metrics: &mut StringMetrics) -> Result<(), Failure> {
        self.costs.escape_sequences = add(self.costs.escape_sequences, 1)?;
        let byte = self
            .peek()
            .ok_or(Failure::Syntax(PrefixErrorKind::UnexpectedEnd))?;
        self.advance()?;
        match byte {
            b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't' => metrics.append(1, 1, 1)?,
            b'u' => {
                let unit = self.hex_unit()?;
                let lookahead = self.offset;
                if (0xD800..=0xDBFF).contains(&unit)
                    && self.input.get(lookahead) == Some(&b'\\')
                    && self.input.get(add(lookahead, 1)?) == Some(&b'u')
                {
                    let tail_start = add(lookahead, 2)?;
                    let tail_end = add(tail_start, 4)?;
                    if let Some(low) = self.input.get(tail_start..tail_end).and_then(decode_hex)
                        && (0xDC00..=0xDFFF).contains(&low)
                    {
                        self.offset = tail_end;
                        self.costs.escape_sequences = add(self.costs.escape_sequences, 1)?;
                        metrics.append(4, 1, 4)?;
                        return Ok(());
                    }
                }
                if (0xD800..=0xDFFF).contains(&unit) {
                    self.costs.unpaired_surrogates = add(self.costs.unpaired_surrogates, 1)?;
                }
                let bytes = if unit < 0x80 {
                    1
                } else if unit < 0x800 {
                    2
                } else {
                    3
                };
                metrics.append(bytes, 1, if bytes == 1 { 1 } else { 4 })?;
            }
            _ => return Err(Failure::Syntax(PrefixErrorKind::InvalidEscape)),
        }
        Ok(())
    }

    fn hex_unit(&mut self) -> Result<u16, Failure> {
        let end = add(self.offset, 4)?;
        let Some(bytes) = self.input.get(self.offset..end) else {
            self.offset = self.input.len();
            return Err(Failure::Syntax(PrefixErrorKind::UnexpectedEnd));
        };
        self.offset = end;
        decode_hex(bytes).ok_or(Failure::Syntax(PrefixErrorKind::InvalidEscape))
    }
}

#[derive(Default)]
struct StringMetrics {
    bytes: usize,
    scalars: usize,
    required: usize,
    escaped: bool,
    invalid_utf8: bool,
}

impl StringMetrics {
    fn append(
        &mut self,
        bytes: usize,
        scalars: usize,
        reserve: usize,
    ) -> Result<(), NumericReject> {
        self.required = self.required.max(add(self.bytes, reserve)?);
        self.bytes = add(self.bytes, bytes)?;
        self.scalars = add(self.scalars, scalars)?;
        Ok(())
    }
}

fn decode_hex(bytes: &[u8]) -> Option<u16> {
    if bytes.len() != 4 {
        return None;
    }
    let mut value = 0u16;
    for &byte in bytes {
        let digit = match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            b'A'..=b'F' => byte - b'A' + 10,
            _ => return None,
        };
        value = value.checked_mul(16)?.checked_add(u16::from(digit))?;
    }
    Some(value)
}
