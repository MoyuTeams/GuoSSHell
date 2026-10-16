use rshell_m0::rshell_core::{ConnectionId, GroupId, TerminalProfileId, TransportKind};

use super::Failure;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum ProfileSource {
    Explicit = 1,
    Connection = 2,
    Default = 3,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldValue<'a> {
    Null,
    Text(&'a str),
    Port(u16),
    Position(i64),
}

/// 字节及 descriptor backing 必须由调用方的 Hraw owner 保持到最后 borrow 结束。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct RawField<'a> {
    pub tag: u16,
    pub ordinal: u32,
    pub value: FieldValue<'a>,
}

#[derive(Clone, Copy, Debug)]
pub struct QuickTarget<'a> {
    /// 准入方已生成的请求身份；decode 不调用随机数或构造默认 profile。
    pub id: ConnectionId,
    pub host: &'a str,
    pub port: u16,
    pub username: &'a str,
    pub command: &'a str,
}

#[derive(Clone, Copy, Debug)]
pub struct Selection<'a> {
    pub connection: Option<ConnectionId>,
    pub explicit_profile: Option<TerminalProfileId>,
    pub expected_transport: TransportKind,
    pub quick: Option<QuickTarget<'a>>,
}

/// 来自已验证 helper snapshot 的关联数字；本类型不认证产物或证明数据库查询。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct RawIdentity {
    pub transaction_nonce: [u8; 16],
    pub revision: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct SelectedReferenceToken {
    pub snapshot: RawIdentity,
    pub selected_connection: Option<ConnectionId>,
}

/// Exists/Absent 的身份按 UUID 二进制值关联，允许旧 reader 的全部文本别名。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum GroupPresence {
    NoReference = 0,
    Exists(GroupId) = 1,
    Absent(GroupId) = 2,
    Unproven = 3,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct SelectedReferences {
    token: SelectedReferenceToken,
    group: GroupPresence,
}

impl SelectedReferences {
    /// 调用方显式提供同 readonly 事务的事实；构造不授予生产 gate 或查询权限。
    pub const fn from_trusted_transaction(
        token: SelectedReferenceToken,
        group: GroupPresence,
    ) -> Self {
        Self { token, group }
    }

    pub const fn token(self) -> SelectedReferenceToken {
        self.token
    }

    pub const fn group(self) -> GroupPresence {
        self.group
    }
}

#[derive(Clone, Copy, Debug)]
pub struct RawFields<'a> {
    fields: &'a [RawField<'a>],
    pub revision: u64,
    pub source: ProfileSource,
    pub selection: Selection<'a>,
    transaction_nonce: [u8; 16],
    references: SelectedReferences,
    first: u16,
}

impl<'a> RawFields<'a> {
    /// framing、同事务及 raw grant 由 helper reader 验证；引用事实必须显式传入。
    /// 引用关联和存在性在字段/overrides codec 后、连接 predicate 前裁决。
    pub fn checked(
        fields: &'a [RawField<'a>],
        identity: RawIdentity,
        source: ProfileSource,
        selection: Selection<'a>,
        references: SelectedReferences,
    ) -> Result<Self, Failure> {
        let saved = selection.connection.is_some();
        if saved == selection.quick.is_some() {
            return Err(Failure::Protocol);
        }
        let first = if saved { 1 } else { 17 };
        let fixed = if saved { 22 } else { 6 };
        if fields.len() < fixed || (!saved && fields.len() != fixed) {
            return Err(Failure::Protocol);
        }
        let mut previous_tag = None;
        for (index, field) in fields.iter().enumerate() {
            if index < fixed {
                if field.tag != first + index as u16 || field.ordinal != 0 {
                    return Err(Failure::Protocol);
                }
                let nullable = matches!(field.tag, 2 | 9 | 10 | 12 | 15);
                let valid = match field.value {
                    FieldValue::Null => nullable,
                    FieldValue::Port(_) => field.tag == 5,
                    FieldValue::Position(_) => field.tag == 14,
                    FieldValue::Text(_) => !matches!(field.tag, 5 | 14),
                };
                if !valid {
                    return Err(Failure::Protocol);
                }
            } else {
                let ordinal = u32::try_from(index - fixed + 1).map_err(|_| Failure::Protocol)?;
                let FieldValue::Text(tag) = field.value else {
                    return Err(Failure::Protocol);
                };
                if field.tag != 23
                    || field.ordinal != ordinal
                    || previous_tag.is_some_and(|previous: &str| previous >= tag)
                {
                    return Err(Failure::Protocol);
                }
                previous_tag = Some(tag);
            }
        }
        Ok(Self {
            fields,
            revision: identity.revision,
            source,
            selection,
            transaction_nonce: identity.transaction_nonce,
            references,
            first,
        })
    }

    pub fn selected_references(&self) -> SelectedReferences {
        self.references
    }

    pub(super) fn validate_references(
        &self,
        decoded_connection: Option<ConnectionId>,
        group: Option<GroupId>,
    ) -> Result<(), Failure> {
        let token = self.references.token;
        if token.snapshot.transaction_nonce != self.transaction_nonce
            || token.snapshot.revision != self.revision
            || token.selected_connection != self.selection.connection
            || decoded_connection != self.selection.connection
        {
            return Err(Failure::Protocol);
        }
        match (group, self.references.group) {
            (None, GroupPresence::NoReference) => Ok(()),
            (Some(id), GroupPresence::Exists(proved)) if id == proved => Ok(()),
            (Some(id), GroupPresence::Absent(proved)) if id == proved => {
                Err(Failure::MissingGroup(id))
            }
            (_, GroupPresence::Unproven) => Err(Failure::ReferenceNotReady),
            _ => Err(Failure::Protocol),
        }
    }

    pub fn saved(&self) -> bool {
        self.first == 1
    }

    pub fn fields(&self) -> &'a [RawField<'a>] {
        self.fields
    }

    pub(super) fn value(&self, tag: u16) -> FieldValue<'a> {
        // checked 的完整固定列集保证这些内部调用的索引有效。
        self.fields[usize::from(tag - self.first)].value
    }

    pub(super) fn text(&self, tag: u16) -> &'a str {
        match self.value(tag) {
            FieldValue::Text(value) => value,
            _ => unreachable!("内部调用只读取已核验的非空文本列"),
        }
    }

    pub(super) fn nullable_text(&self, tag: u16) -> Option<&'a str> {
        match self.value(tag) {
            FieldValue::Text(value) => Some(value),
            FieldValue::Null => None,
            _ => unreachable!("内部调用只读取已核验的 nullable 文本列"),
        }
    }

    pub fn tags(&self) -> impl Iterator<Item = &'a str> + '_ {
        self.fields.iter().filter_map(|field| {
            if field.tag == 23
                && let FieldValue::Text(text) = field.value
            {
                return Some(text);
            }
            None
        })
    }
}
