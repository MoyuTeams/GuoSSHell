//! fixture 的已授额文本所有权；固定票据槽不保存无限完成历史。

use super::core::{BUSY, EXHAUSTED, INVALID, STALE_OWNER, TEXT_CAPACITY};

pub(super) const TICKETS: usize = 4;
pub(super) const PAYLOAD_LIMIT: usize = 4096;
pub(super) const RETAINED_LIMIT: usize = 8192;
pub(super) const CHUNK: usize = 32;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct TicketRef {
    pub owner: u64,
    pub context: u64,
    pub generation: u64,
    pub transaction: u64,
    pub effect: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct PayloadGrant {
    pub id: u64,
    pub total: u32,
    pub chunk: u32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Phase {
    Free,
    Open,
    Committed,
    Cancelled,
    Done,
}

pub(super) struct Ticket {
    pub reference: TicketRef,
    pub phase: Phase,
    pub paste: bool,
    pub revision: u64,
    pub grant: Option<PayloadGrant>,
    pub text: String,
    pub final_chunk: bool,
    pub preedit: [u8; PAYLOAD_LIMIT],
    pub preedit_len: usize,
}

impl Default for Ticket {
    fn default() -> Self {
        Self {
            reference: TicketRef::default(),
            phase: Phase::Free,
            paste: false,
            revision: 0,
            grant: None,
            text: String::new(),
            final_chunk: false,
            preedit: [0; PAYLOAD_LIMIT],
            preedit_len: 0,
        }
    }
}

pub(super) struct Payloads {
    pub tickets: [Ticket; TICKETS],
    pub retained: usize,
    next_grant: u64,
}

impl Default for Payloads {
    fn default() -> Self {
        Self {
            tickets: std::array::from_fn(|_| Ticket::default()),
            retained: 0,
            next_grant: 1,
        }
    }
}

impl Payloads {
    pub fn available(&self) -> Option<usize> {
        self.tickets
            .iter()
            .position(|t| matches!(t.phase, Phase::Free | Phase::Done))
    }

    pub fn open(
        &mut self,
        slot: usize,
        owner: u64,
        context: u64,
        effect: u64,
        paste: bool,
    ) -> Result<TicketRef, u32> {
        let generation = self.tickets[slot]
            .reference
            .generation
            .checked_add(1)
            .ok_or(EXHAUSTED)?;
        let reference = TicketRef {
            owner,
            context,
            generation,
            transaction: effect,
            effect,
        };
        self.tickets[slot] = Ticket {
            reference,
            phase: Phase::Open,
            paste,
            ..Ticket::default()
        };
        Ok(reference)
    }

    pub fn find(&self, reference: TicketRef) -> Result<usize, u32> {
        self.tickets
            .iter()
            .position(|t| t.phase != Phase::Free && t.reference == reference)
            .ok_or(STALE_OWNER)
    }

    pub fn grant(&mut self, reference: TicketRef, total: usize) -> Result<PayloadGrant, u32> {
        let slot = self.find(reference)?;
        let t = &mut self.tickets[slot];
        if t.phase != Phase::Open || t.grant.is_some() {
            return Err(INVALID);
        }
        if total > PAYLOAD_LIMIT {
            return Err(TEXT_CAPACITY);
        }
        if self.retained.checked_add(total).ok_or(EXHAUSTED)? > RETAINED_LIMIT {
            return Err(BUSY);
        }
        let next = self.next_grant.checked_add(1).ok_or(EXHAUSTED)?;
        let grant = PayloadGrant {
            id: self.next_grant,
            total: total as u32,
            chunk: CHUNK as u32,
        };
        // 先授额，后拥有复制容量；输出编码另由 cursor 的工作区预算承担。
        self.retained += total;
        self.next_grant = next;
        t.grant = Some(grant);
        t.text = String::with_capacity(total);
        Ok(grant)
    }

    pub fn chunk(
        &mut self,
        reference: TicketRef,
        grant: PayloadGrant,
        offset: usize,
        bytes: &[u8],
        final_chunk: bool,
    ) -> Result<(), u32> {
        let slot = self.find(reference)?;
        let t = &mut self.tickets[slot];
        let text = std::str::from_utf8(bytes).map_err(|_| INVALID)?;
        if t.phase != Phase::Open
            || t.grant != Some(grant)
            || t.final_chunk
            || offset != t.text.len()
            || bytes.len() > CHUNK
            || offset.checked_add(bytes.len()).ok_or(EXHAUSTED)? > grant.total as usize
        {
            return Err(INVALID);
        }
        t.text.push_str(text);
        t.final_chunk = final_chunk;
        Ok(())
    }

    pub fn update(
        &mut self,
        reference: TicketRef,
        revision: u64,
        preedit: &[u8],
    ) -> Result<(), u32> {
        let slot = self.find(reference)?;
        let t = &mut self.tickets[slot];
        std::str::from_utf8(preedit).map_err(|_| INVALID)?;
        if preedit.len() > PAYLOAD_LIMIT {
            return Err(TEXT_CAPACITY);
        }
        if t.phase != Phase::Open || revision <= t.revision {
            return Err(INVALID);
        }
        t.preedit[..preedit.len()].copy_from_slice(preedit);
        t.preedit_len = preedit.len();
        t.revision = revision;
        Ok(())
    }

    pub fn finish(&mut self, reference: TicketRef, revision: u64) -> Result<bool, u32> {
        let slot = self.find(reference)?;
        let t = &mut self.tickets[slot];
        if matches!(t.phase, Phase::Committed | Phase::Done) && t.revision == revision {
            return Ok(false);
        }
        if t.phase != Phase::Open || !t.final_chunk || revision <= t.revision {
            return Err(INVALID);
        }
        t.revision = revision;
        t.phase = Phase::Committed;
        t.preedit_len = 0;
        Ok(true)
    }

    pub fn cancel(&mut self, reference: TicketRef) -> Result<(), u32> {
        let slot = self.find(reference)?;
        if self.tickets[slot].phase != Phase::Open {
            return Err(INVALID);
        }
        self.release(slot);
        self.tickets[slot].phase = Phase::Cancelled;
        Ok(())
    }

    pub fn release(&mut self, slot: usize) {
        let t = &mut self.tickets[slot];
        if let Some(grant) = t.grant.take() {
            self.retained -= grant.total as usize;
        }
        t.text = String::new();
        t.preedit_len = 0;
    }

    pub fn discard(&mut self) {
        for slot in 0..TICKETS {
            self.release(slot);
            self.tickets[slot].phase = Phase::Done;
        }
    }
}

/// 私有 paste adapter 保留现有换行／控制字符过滤语义；整个事务仅一次包装。
pub(super) fn paste(text: &str, bracketed: bool) -> Vec<u8> {
    let mut body = String::with_capacity(text.len() + 12);
    if bracketed {
        body.push_str("\x1b[200~");
    }
    let start = body.len();
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                body.push('\r');
            }
            '\n' => body.push('\r'),
            '\t' => body.push('\t'),
            ch if ch.is_control() => {}
            ch => body.push(ch),
        }
    }
    if body.len() == start {
        body.clear();
    } else if bracketed {
        body.push_str("\x1b[201~");
    }
    body.into_bytes()
}
