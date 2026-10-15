//! 有限源式见同目录 SOURCE.md；诊断只互核源式，不从调用后大小反推额度。

use std::alloc::Layout;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::task::Waker;

use rshell_m0::rshell_core::{
    ConnectionProfile, InteractionId, InteractionRequest, InteractionResponse,
};
use rshell_m0::rshell_session::{AuthPlan, AuthPlanError, InteractionBroker};
use tokio::sync::{mpsc, oneshot};

use super::operation::{Opcode, OperationBound, OperationRoot};
use crate::selected_p1::model::layout::{NumericReject, add, align_up, btree_nodes, extend, mul};
use crate::selected_p1::resource::ledger::{Admission, ProcessLedger};
use crate::selected_p1::resource::secret::{LockedSecretString, SecretOperation};

pub type LockedSecretOperation<'a> =
    OperationRoot<'a, SecretOperation<'a, Result<AuthPlan, AuthPlanError>>>;
pub type InteractionMessage = (InteractionId, InteractionRequest);
pub type PendingResponses = BTreeMap<InteractionId, oneshot::Sender<InteractionResponse>>;

pub fn arc_layout(payload: Layout) -> Result<Layout, NumericReject> {
    Ok(extend(
        Layout::array::<usize>(2).map_err(|_| NumericReject::InvalidLayout)?,
        payload,
    )?
    .0)
}

/// exact std Arc + Tokio oneshot::Inner：state/Option<T>/两个 Task(Waker)。
pub fn oneshot_upper<T>() -> Result<Layout, NumericReject> {
    let fields = [
        Layout::new::<usize>(),
        Layout::new::<Option<T>>(),
        Layout::new::<Waker>(),
        Layout::new::<Waker>(),
    ];
    let align = fields.iter().map(Layout::align).max().unwrap_or(1);
    let mut bytes = 0;
    for field in fields {
        bytes = add(
            bytes,
            crate::selected_p1::model::layout::align_up(field.size(), align)?,
        )?;
    }
    arc_layout(Layout::from_size_align(bytes, align).map_err(|_| NumericReject::InvalidLayout)?)
}

/// Chan 仍需其 private 字段/actual cfg 的独立源上界，不能用 Sender 指针代替。
pub fn mpsc_block_upper<T>() -> Result<Layout, NumericReject> {
    let header = Layout::array::<usize>(4).map_err(|_| NumericReject::InvalidLayout)?;
    let values = Layout::array::<T>(32).map_err(|_| NumericReject::InvalidLayout)?;
    // Header/Values 两种字段次序均枚举，保持 repr(Rust) 上界。
    let a = extend(header, values)?.0;
    let b = extend(values, header)?.0;
    Ok(if a.size() > b.size() { a } else { b })
}

/// GNU x64、非 loom/tracing 的 Chan 字段上界。这里只展开字段布局，不搬私有类型。
pub fn bounded_chan_upper() -> Result<Layout, NumericReject> {
    if !cfg!(all(
        target_os = "windows",
        target_env = "gnu",
        target_arch = "x86_64"
    )) {
        return Err(NumericReject::UnsupportedEnvelope);
    }
    // cachepad Tx(2 words)、AtomicWaker(usize+Option<Waker>)；Notify 使用实际 public。
    // batch Semaphore=Mutex<Waitlist{2 ptr,bool}>+usize；bounded 再加 bound usize。
    // Mutex 的 OS primitive/niche 由真实 public Mutex tuple 布局取得，非复制 SDK struct。
    let waitlist = Layout::new::<
        Mutex<(
            Option<std::ptr::NonNull<()>>,
            Option<std::ptr::NonNull<()>>,
            bool,
        )>,
    >();
    let semaphore = add(waitlist.size(), 2 * Layout::new::<usize>().size())?;
    let rx_fields = align_up(3 * Layout::new::<usize>().size() + 1, 8)?;
    let fields = [
        128,
        128,
        Layout::new::<tokio::sync::Notify>().size(),
        semaphore,
        8,
        8,
        rx_fields,
    ];
    let mut bytes = 0;
    for field in fields {
        bytes = add(bytes, align_up(field, 128)?)?;
    }
    Layout::from_size_align(bytes, 128).map_err(|_| NumericReject::InvalidLayout)
}

pub fn auth_plan_bound(profile: &ConnectionProfile) -> Result<OperationBound, NumericReject> {
    // from_secret 成功仍 eager 构造 CredentialMissing(host.clone()) 后 drop，故两份 host。
    // unsupported/error 分支和 identity clone 保守相加；不包含调用前已带额度的 secret。
    let path = profile
        .identity_file
        .as_ref()
        .map_or(0, |path| path.as_os_str().as_encoded_bytes().len());
    let bytes = add(mul(profile.host.len(), 2)?, path)?;
    Ok(OperationBound {
        opcode: Opcode::AuthPlan,
        rust_bytes: bytes,
    })
}

/// 不调用 Native transport，不把 SecretOperation 借出到 task；唯一秘密 backing move。
pub fn prepare_auth_plan<'a>(
    profile: &ConnectionProfile,
    secret: LockedSecretString<'a>,
    ledger: &'a ProcessLedger,
    admission: Admission,
) -> Result<LockedSecretOperation<'a>, NumericReject> {
    let bound = auth_plan_bound(profile)?;
    // SAFETY: AuthPlan::from_secret 只返回 owned plan/error，无 spawn/回调，secret 无 clone；
    // 返回值连 error 都被 root 持有到真实 drop，额外 host/path 先 reserve。
    unsafe {
        OperationRoot::before_call(bound, ledger, admission, || {
            secret.into_operation(|secret| AuthPlan::from_secret(profile, Some(secret)))
        })
    }
}

/// 每个新请求先预约 oneshot + pending insert 峰值；body/错误字符串另由 caller 源式。
/// pending 节点归 channel 内 PendingResponses 所有，remove 最后 entry 仍可能保留空叶根。
/// 返回 bound 的节点额度必须由实际 channel 所有者持有到 pending map 及其全部
/// Arc/回调 holder 的 backing 真实析构之后；request/reply 完成或 future drop 不可归还。
pub fn interaction_request_bound(
    pending_occurrences: usize,
    request_graph: usize,
) -> Result<OperationBound, NumericReject> {
    let nodes = btree_nodes(
        Layout::new::<InteractionId>(),
        Layout::new::<oneshot::Sender<InteractionResponse>>(),
        8,
    )?;
    let n = add(pending_occurrences, 1)?;
    // 有限 insert 上界：每 5 keys 最多一个节点，split 在途最多 h+2；以 n+2 更保守覆盖。
    // tree 的额度跨 request 留在 channel 所有者，直到 PendingResponses 真实析构。
    let tree = mul(add(n, 2)?, nodes.internal.size())?;
    OperationBound::sum(
        Opcode::InteractionRequest,
        &[oneshot_upper::<InteractionResponse>()?],
        add(tree, request_graph)?,
    )
}

/// 只接 source 展开后的 Chan 上界；源码图所有 broker/request/receiver 不可逃逸。
pub fn interaction_channel_bound(
    chan_upper: Layout,
    live_blocks: usize,
) -> Result<OperationBound, NumericReject> {
    if live_blocks == 0 {
        return Err(NumericReject::UnsupportedEnvelope);
    }
    OperationBound::sum(
        Opcode::InteractionChannel,
        &[
            arc_layout(chan_upper)?,
            arc_layout(Layout::new::<Mutex<PendingResponses>>())?,
        ],
        mul(
            mpsc_block_upper::<InteractionMessage>()?.size(),
            live_blocks,
        )?,
    )
}

pub type InteractionParts = (InteractionBroker, mpsc::Receiver<InteractionMessage>);

/// # Safety
/// chan_upper 必须由 locked Chan/sem/Notify/RxFields/cachepadding 源式和真实诊断互核；
/// live_blocks 覆盖 initial、close 及所有插入竞争。parts 不得 clone/移出 root，
/// 各 request future/回调先 drop，然后 broker 和 receiver 都 drop 后再释放 root。
/// 此 channel bound 只含空 BTreeMap 的 inline/header，不含后续 insert 的节点。
/// caller 须将 interaction_request_bound 的节点 grants 保留到 PendingResponses
/// 及所有实际 holder 真实析构之后，不能以 pending.is_empty()/reply done 提前归还。
pub unsafe fn create_interaction_root<'a>(
    chan_upper: Layout,
    live_blocks: usize,
    ledger: &'a ProcessLedger,
    admission: Admission,
) -> Result<OperationRoot<'a, InteractionParts>, NumericReject> {
    unsafe {
        OperationRoot::before_call(
            interaction_channel_bound(chan_upper, live_blocks)?,
            ledger,
            admission,
            rshell_m0::rshell_session::interaction_channel,
        )
    }
}

/// 公共 layout 只作 actual 类型互核，不把 Arc/Box 指针尺寸计成 pointee。
pub fn public_layouts() -> [Layout; 8] {
    [
        Layout::new::<AuthPlan>(),
        Layout::new::<InteractionBroker>(),
        Layout::new::<InteractionMessage>(),
        Layout::new::<InteractionResponse>(),
        Layout::new::<oneshot::Sender<InteractionResponse>>(),
        Layout::new::<mpsc::Receiver<InteractionMessage>>(),
        Layout::new::<Arc<Mutex<PendingResponses>>>(),
        Layout::new::<[Waker; 2]>(),
    ]
}
