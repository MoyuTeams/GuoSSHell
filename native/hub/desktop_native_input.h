#ifndef GUOSSH_DESKTOP_NATIVE_INPUT_H
#define GUOSSH_DESKTOP_NATIVE_INPUT_H

#include <stdint.h>
#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

/* 独立 normal ABI；不是 spike ABI，不能通过 Dart 数值接口暴露登记能力。
 * 所有结构采用平台默认 C 对齐，不使用 pack。输入及输出预填 version=1、
 * size=sizeof(对应结构)，不接受未知版本或不匹配的完整布局。
 * 指针须在调用期间真实有效、正确对齐；UTF-8 不带 NUL，长度单位为字节。
 * Rust 只检查 null/长度/溢出/UTF-8，不能验证任意地址是否可访问。
 * 失败不修改输出、buffer 或 count。任何 handle 都是不可 retarget 的 opaque ID，
 * 不可解引用，不复用；每次调用持有原 Surface Arc。
 */
#define GUOSSH_NORMAL_ABI_VERSION 1u
#define GUOSSH_NORMAL_WINDOWS_INPUT_PROPERTY L"GuoSSHell.NormalInput.V1"

typedef enum GuosshNormalStatus {
    GUOSSH_NORMAL_OK = 0, GUOSSH_NORMAL_INVALID_ARGUMENT = 1,
    GUOSSH_NORMAL_UNSUPPORTED_VERSION = 2, GUOSSH_NORMAL_UNKNOWN_HANDLE = 3,
    GUOSSH_NORMAL_NOT_ATTACHED = 4, GUOSSH_NORMAL_STALE_SOURCE = 5,
    GUOSSH_NORMAL_STALE_ATTEMPT = 6, GUOSSH_NORMAL_STALE_FOCUS = 7,
    GUOSSH_NORMAL_NOT_AUTHENTICATED = 8, GUOSSH_NORMAL_WRONG_THREAD = 9,
    GUOSSH_NORMAL_RETIRED = 10, GUOSSH_NORMAL_BUSY = 11,
    GUOSSH_NORMAL_TOO_LARGE = 12, GUOSSH_NORMAL_MIXED_MODE = 13,
    GUOSSH_NORMAL_DUPLICATE = 14, GUOSSH_NORMAL_INVALID_EVENT = 15,
    GUOSSH_NORMAL_INVALID_TICKET = 16, GUOSSH_NORMAL_CANCELLED = 17,
    GUOSSH_NORMAL_POLICY_REJECTED = 18, GUOSSH_NORMAL_BUFFER_TOO_SMALL = 19,
    GUOSSH_NORMAL_UNSUPPORTED_PLATFORM = 20, GUOSSH_NORMAL_INTERNAL = 21
} GuosshNormalStatus;

typedef struct GuosshNormalHeader { uint32_t version; uint32_t size; } GuosshNormalHeader;

/* identity 是原 Prepared 身份，不是 current pane 的查找提示。
 * bind 首次查找 handle=0；非零 handle 必须仍属于同一原 surface。
 */
typedef struct GuosshNormalContext {
    GuosshNormalHeader header;
    uint64_t handle;
    uint32_t session_id;
    uint64_t generation;
    const uint8_t *identity;
    uint64_t identity_len;
    uint64_t attempt;
    uint64_t focus_epoch;
} GuosshNormalContext;

typedef struct GuosshNormalRegistration {
    GuosshNormalHeader header;
    uint32_t session_id;
    uint64_t generation;
    const uint8_t *identity;
    uint64_t identity_len;
    uint64_t attempt;
    uint64_t window;
    uint64_t input_object;
} GuosshNormalRegistration;

typedef struct GuosshNormalBound {
    GuosshNormalHeader header;
    uint64_t handle;
    uint64_t attempt;
    uint64_t focus_epoch;
} GuosshNormalBound;

/* kind: 1=Unicode scalar，2=Enter，3=Escape，4=Tab，5=Backspace，6=Delete，
 * 7=Insert，8=Home，9=End，10=PageUp，11=PageDown，12=Up，13=Down，
 * 14=Left，15=Right，16=F1..F24。只有 kind=1/16 的 value 非零。
 * phase: 1=Down，2=Repeat，3=Up。
 * modifiers: Shift=1，Control=2，Alt=4，Super=8，LeftAlt=16，RightAlt=32。
 * raw_known/same_event_dual: 0=缺失，1=宿主已证实；这些不是自报即可签发的证明。
 * alt_provenance: 0=Unknown，1=Known；text_ownership: 0=Unverified，
 * 1=TerminalKey，2=NativeText。Unknown/缺失/歧义由既有策略拒绝，不能退成 IME。
 * default_action: 0=无；其余采用下列 action 编号。
 * context_id 是宿主原 editable context 的非零 ID。Start 在原 binding 内递增且
 * resume_text_ticket=0；Continue 必须是同 Open context 的原 ticket，不是新 effect。
 * sealed context 不复用；context-local 轮换不 revoke surface、不清 physical。
 * Up 只释放原 press；只有 Admission.route=NativeText 且 text_op=None 才可送原 TSF Up。
 */
typedef struct GuosshNormalKey {
    GuosshNormalHeader header;
    uint64_t physical;
    uint32_t phase, modifiers;
    uint32_t raw_kind, raw_value;
    uint32_t code_kind, code_value, legacy_kind, legacy_value;
    uint32_t raw_known, same_event_dual, alt_provenance;
    uint32_t text_ownership, default_action;
    uint64_t context_id, resume_text_ticket;
} GuosshNormalKey;

/* route: 1=Terminal，2=NativeText，3=Consumed。
 * text_op: 0=None，1=Start，2=Continue，3=End。Continue 保留原 ticket/effect。
 * cycle_id 是本物理按压；Up 不创建文本。End 的三个 end_* 字段只指原 context。
 */
typedef struct GuosshNormalAdmission {
    GuosshNormalHeader header;
    uint64_t effect, text_ticket;
    uint32_t route;
    uint64_t cycle_id;
    uint32_t text_op;
    uint64_t end_fence, end_context_id, end_transaction_id;
} GuosshNormalAdmission;

typedef struct GuosshNormalTextEnd {
    GuosshNormalHeader header;
    uint64_t context_id, transaction_id, end_fence;
} GuosshNormalTextEnd;

typedef struct GuosshNormalGeometry {
    uint32_t frame_revision;
    uint64_t row_revision;
    int64_t viewport_top;
    uint64_t font_revision, layout_revision, transform_revision;
    double clip_x, clip_y, clip_width, clip_height;
    double transform[6];
} GuosshNormalGeometry;
typedef struct GuosshNormalPresentedGeometry {
    GuosshNormalHeader header;
    uint64_t native_request_id;
    GuosshNormalGeometry geometry;
} GuosshNormalPresentedGeometry;
/* kind: 1=press，2=move，3=release，4=wheel；button: 0=none，1=left，2=middle，3=right。
 * modifiers: Shift=1，Control=2，Alt=4，Super=8；buttons: Left=1，Middle=2，Right=4。
 * remote 坐标为零基屏幕单元；delta_y 为有符号行数，正数向下，绝对值最多64。
 * 非 wheel 的 delta 为零；当前终端水平滚轮不编码，非零 delta_x 明确拒绝。
 * 几何必须与原 presented 完全一致。Busy 不消费 ID，暂停后续 key/geometry，原样重投。
 */
typedef struct GuosshNormalFrozenPointer {
    GuosshNormalGeometry geometry;
    uint32_t kind, button, modifiers, buttons;
    int64_t stable_hit_row;
    uint32_t column, remote_column, remote_row;
    int32_t delta_x, delta_y;
} GuosshNormalFrozenPointer;
typedef struct GuosshNormalPointer {
    GuosshNormalHeader header;
    uint64_t native_request_id;
    GuosshNormalFrozenPointer input;
} GuosshNormalPointer;

/* kind: 1=preedit，2=commit，3=cancel（cancel 要求 text_len=0）。
 * IME 只能使用实际事件返回的原票据，不能为迟到回调重取 current pane。
 */
typedef struct GuosshNormalComposition {
    GuosshNormalHeader header;
    uint64_t text_ticket;
    uint32_t kind;
    const uint8_t *text;
    uint64_t text_len;
} GuosshNormalComposition;
typedef struct GuosshNormalTextReservation {
    GuosshNormalHeader header;
    uint64_t text_ticket, utf8_bytes;
} GuosshNormalTextReservation;
typedef struct GuosshNormalTextChunk {
    GuosshNormalHeader header;
    uint64_t text_ticket, offset;
    const uint8_t *bytes;
    uint64_t len;
} GuosshNormalTextChunk;
typedef struct GuosshNormalTextFinish {
    GuosshNormalHeader header;
    uint64_t text_ticket;
} GuosshNormalTextFinish;

/* action: 1=NewTab，2=SplitRight，3=SplitDown，4=ClosePane，5=NextTab，
 * 6=PreviousTab，7=NextPane，8=Search，9=Settings，10=ToggleSidebar，
 * 11=Copy，12=Paste，13=SelectAll，14=ZoomIn，15=ZoomOut，16=ZoomReset。
 * request_id 与现有 Rinf NativeCommand 共用来源内单调去重空间。
 * resize/viewport/显式selection/paste/frameACK/连接控制沿现有 Rinf。
 * 物理 pointer 与 presented geometry 走下列同步入口及独立 native counter；
 * 准入后仍在同一 InputPort effect 顺序内，不由 Dart 伪装物理来源。
 */
typedef struct GuosshNormalActionRequest {
    GuosshNormalHeader header;
    uint64_t request_id;
    uint32_t action;
} GuosshNormalActionRequest;
typedef struct GuosshNormalClaimRequest { GuosshNormalHeader header; uint64_t effect; } GuosshNormalClaimRequest;
typedef struct GuosshNormalClaimInfo {
    GuosshNormalHeader header;
    uint64_t claim, effect;
    uint32_t action, rejection;
    uint64_t payload_len;
} GuosshNormalClaimInfo;
typedef struct GuosshNormalPayloadCopy {
    GuosshNormalHeader header;
    uint64_t claim;
    uint8_t *buffer;
    uint64_t capacity;
} GuosshNormalPayloadCopy;

/* kind: 1=Completed，2=明确 abort，3=Paste 的实际完整文本。
 * Completed/abort 要求 text_len=0。返回 effect/投递旧信号不是消费者完成。
 */
typedef struct GuosshNormalAck {
    GuosshNormalHeader header;
    uint64_t claim;
    uint32_t kind;
    const uint8_t *text;
    uint64_t text_len;
} GuosshNormalAck;

/* 来自已发布的同一 RenderFrame，不触发 render。caret 坐标是 viewport 相对
 * 像素格矩形，不是 GUI 全局坐标；C++ 负责真实布局偏移、剪裁与 OS 坐标转换。
 * *_present=0 时对应字段为零。没有已发布投影返回 NotAttached。
 */
typedef struct GuosshNormalProjection {
    GuosshNormalHeader header;
    uint32_t frame_revision;
    uint64_t applied_effect, row_revision;
    uint32_t follow_bottom;
    int64_t first_stable_row, screen_top_stable_row, viewport_top;
    uint32_t mouse_reporting, bracketed_paste, selection_present;
    int64_t start_row;
    uint32_t start_col;
    int64_t end_row;
    uint32_t end_col, rectangular, caret_present;
    double caret_x, caret_y, caret_width, caret_height;
    uint32_t dpi;
} GuosshNormalProjection;

/* UNSAFE trusted-host 合同：仅真实原生适配器调用。
 * Windows 宿主在实际输入 HWND 上 SetPropW(PROPERTY, 实际 input_object)，
 * 对象必须真实存活并排他关联精确 Root Source/identity/attempt。Rust 核验 HWND、
 * 当前进程/UI 线程、GetFocus 与 GetPropW 的对象关联，不解引用该对象。
 * 这些 OS 核验不证明 raw/dual/physical/IME 同事件；宿主必须从实际原生事件及
 * 文本服务证明它们、维持排他焦点域，并让真实 callbacks 持有原 handle。
 * Dart/Rinf 数值、bool 或 fixture 不能满足此前提。普通 bind 不执行登记。
 * macOS/Linux 暂无真实 producer，登记返回 UnsupportedPlatform；未附接 bind 拒绝。
 */
uint32_t guossh_normal_v1_register_trusted_host(const GuosshNormalRegistration *, GuosshNormalBound *);
uint32_t guossh_normal_v1_bind(const GuosshNormalContext *, GuosshNormalBound *);
uint32_t guossh_normal_v1_key(const GuosshNormalContext *, const GuosshNormalKey *, GuosshNormalAdmission *);
uint32_t guossh_normal_v1_composition(const GuosshNormalContext *, const GuosshNormalComposition *);
/* 先授额，再按连续 byte offset 收齐，finish 仅一次原子 commit；失败不截断。 */
uint32_t guossh_normal_v1_reserve_payload(const GuosshNormalContext *, const GuosshNormalTextReservation *);
uint32_t guossh_normal_v1_payload(const GuosshNormalContext *, const GuosshNormalTextChunk *);
uint32_t guossh_normal_v1_finish(const GuosshNormalContext *, const GuosshNormalTextFinish *);
/* query 在无待决 fence 时返回三个零。ack 只结算原 quiet，撤权后仍有效。 */
uint32_t guossh_normal_v1_text_end_query(const GuosshNormalContext *, GuosshNormalTextEnd *);
uint32_t guossh_normal_v1_text_end_ack(const GuosshNormalContext *, const GuosshNormalTextEnd *);
uint32_t guossh_normal_v1_presented_geometry(const GuosshNormalContext *, const GuosshNormalPresentedGeometry *);
uint32_t guossh_normal_v1_invalidate_geometry(const GuosshNormalContext *, uint64_t native_request_id);
uint32_t guossh_normal_v1_pointer(const GuosshNormalContext *, const GuosshNormalPointer *, GuosshNormalAdmission *);
uint32_t guossh_normal_v1_action(const GuosshNormalContext *, const GuosshNormalActionRequest *, uint64_t *effect);
uint32_t guossh_normal_v1_claim(const GuosshNormalContext *, const GuosshNormalClaimRequest *, GuosshNormalClaimInfo *);

/* null buffer + capacity=0 查询实际完整 UTF-8 长度；copy 使用实际可写容量，
 * 不截断、不写 NUL，短容量失败不改 buffer/count，不消费 claim。
 */
uint32_t guossh_normal_v1_action_payload(const GuosshNormalContext *, const GuosshNormalPayloadCopy *, uint64_t *count);
uint32_t guossh_normal_v1_ack(const GuosshNormalContext *, const GuosshNormalAck *);
uint32_t guossh_normal_v1_projection(const GuosshNormalContext *, GuosshNormalProjection *);

/* revoke 只封原 input surface，不断开 SSH；原 attempt 的 Committed delivery 保留。
 * Waiting/Preedit 进入 Ending；原 quiet fence ACK 后才 Cancelled。revoke 不证明 nativequiet，
 * nativequiet 不等 GUI claim、SSH write 或 applied_effect；release 是独立的最后 root 释放。
 * 已 claim 的原动作仍可 copy/Completed/abort，但撤权后的 Paste ACK 不签发新输入。
 * release_retired 的 UNSAFE 合同：原真实 key/IME/COM callbacks 全部退休，
 * claim 已实际 ACK 或明确 abort，先 revoke 后 release；不得 UI join 或 unload hub。
 * callback clone 撤权后失效，只引用原 Surface，绝不迁移至新 pane。
 */
uint32_t guossh_normal_v1_revoke(const GuosshNormalContext *);
uint32_t guossh_normal_v1_release_retired(const GuosshNormalContext *);

#ifdef __cplusplus
}
#endif
#endif
