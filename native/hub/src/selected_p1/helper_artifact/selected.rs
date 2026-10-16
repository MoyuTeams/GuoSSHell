use crate::selected_p1::{
    domain_decode::{
        self, ExecutionCounts, FrozenBuild,
        raw::{
            FieldValue, GroupPresence, ProfileSource, RawField, RawFields, RawIdentity,
            SelectedReferenceToken, SelectedReferences, Selection,
        },
    },
    helper_runtime::{
        process::{BorrowedValue, GroupReferenceFact},
        worker::LoadedRaw,
    },
    model::{certificate::TargetLayouts, layout::NumericReject},
    resource::{
        ledger::{Admission, Amount, ProcessLedger, ResourceClass},
        owner::{OwnerLayout, SharedOwner},
    },
};
use rshell_m0::rshell_core::{ConnectionId, GroupId};
use uuid::Uuid;

/// descriptor 解释 backing 借用 LoadedRaw 的 payload，不复制字段正文。
/// consumer 借用 fields 构造 RawFields；permit 持到 descriptors 的真实 dealloc。
#[repr(C)]
pub struct Fields<'a> {
    fields: Vec<RawField<'a>>,
    identity: RawIdentity,
    source: ProfileSource,
    selection: Selection<'a>,
    references: SelectedReferences,
}
impl<'a> Fields<'a> {
    pub fn raw(&self) -> Result<RawFields<'_>, domain_decode::Failure> {
        RawFields::checked(
            &self.fields,
            self.identity,
            self.source,
            self.selection,
            self.references,
        )
    }
}

/// InvalidUuid/Unproven 不抢先返回；连接 UUID/overrides codec 的旧错误顺序保留。
pub fn project_loaded<'r, 'a>(
    loaded: &'a LoadedRaw,
    selection: Selection<'a>,
    root: &'r ProcessLedger,
    admission: Admission,
) -> Result<SharedOwner<'r, Fields<'a>>, domain_decode::Failure> {
    let facts = loaded.required_facts();
    let connection = facts
        .selected_connection()
        .map(|bytes| ConnectionId(Uuid::from_bytes(bytes)));
    if connection != selection.connection {
        return Err(domain_decode::Failure::Protocol);
    }
    let source = match facts.source() {
        1 => ProfileSource::Explicit,
        2 => ProfileSource::Connection,
        3 => ProfileSource::Default,
        _ => return Err(domain_decode::Failure::Protocol),
    };
    let identity = RawIdentity {
        transaction_nonce: facts.transaction_nonce(),
        revision: facts.revision(),
    };
    let group = match facts.group_reference() {
        GroupReferenceFact::NoReference => GroupPresence::NoReference,
        GroupReferenceFact::Exists | GroupReferenceFact::Absent => {
            let field = loaded.field(1).ok_or(domain_decode::Failure::Protocol)?;
            let BorrowedValue::Text(text) = field.value else {
                return Err(domain_decode::Failure::Protocol);
            };
            let id = GroupId(Uuid::parse_str(text).map_err(|_| domain_decode::Failure::Protocol)?);
            if facts.group_reference() == GroupReferenceFact::Exists {
                GroupPresence::Exists(id)
            } else {
                GroupPresence::Absent(id)
            }
        }
        GroupReferenceFact::InvalidUuid | GroupReferenceFact::Unproven => GroupPresence::Unproven,
    };
    let references = SelectedReferences::from_trusted_transaction(
        SelectedReferenceToken {
            snapshot: identity,
            selected_connection: connection,
        },
        group,
    );
    let count = loaded.storage().descriptors().len();
    let bytes = std::alloc::Layout::array::<RawField<'a>>(count)
        .map_err(|_| NumericReject::InvalidLayout)?
        .size();
    let layout = OwnerLayout::actual::<Fields<'a>>(0)?;
    let grant = root.try_reserve(
        Amount {
            rust_bytes: layout.required_rust(bytes)?,
            c_bytes: 0,
            slots: 0,
        },
        ResourceClass::Raw,
        admission,
    )?;
    Ok(SharedOwner::try_init(grant, 0, bytes, |_| {
        let mut fields = Vec::new();
        fields
            .try_reserve_exact(count)
            .map_err(|_| NumericReject::AllocationFailure)?;
        for index in 0..count {
            let field = loaded
                .field(index)
                .ok_or(NumericReject::ConstructionFailure)?;
            fields.push(RawField {
                tag: field.tag,
                ordinal: field.ordinal,
                value: match field.value {
                    BorrowedValue::Null => FieldValue::Null,
                    BorrowedValue::Text(text) => FieldValue::Text(text),
                    BorrowedValue::Port(value) => FieldValue::Port(value),
                    BorrowedValue::Position(value) => FieldValue::Position(value),
                },
            });
        }
        Ok(Fields {
            fields,
            identity,
            source,
            selection,
            references,
        })
    })?)
}

/// 实际 LoadedRaw 在整个解码中仍由调用方持有；返回的是完整领域 graph owner。
pub fn decode_loaded<'r>(
    loaded: &LoadedRaw,
    selection: Selection<'_>,
    target: &TargetLayouts,
    root: &'r ProcessLedger,
    admission: Admission,
    calls: &mut ExecutionCounts,
) -> Result<FrozenBuild<'r>, domain_decode::Failure> {
    let projected = project_loaded(loaded, selection, root, admission)?;
    let raw = projected.value().raw()?;
    domain_decode::decode_resolve(&raw, target, root, admission, calls)
}
