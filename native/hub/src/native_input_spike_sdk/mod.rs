//! 仅供独立 SDK fixture 的启动隔离与同 module 握手，不创建输入 owner。

use std::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, Ordering};

pub const REQUEST_LEN: usize = 32;
pub const RESPONSE_LEN: usize = 48;
pub const REPLY_SIGNAL: &str = "GuoNativeInputSdkProbeReply";
pub const MAX_RESPONSE_ATTEMPTS: u32 = 8;
const VERSION: u16 = 1;
const CORE_ABI: u32 = 2;

// Atomic 存储确实可变，地址不依赖可能合并的只读常量。
static MODULE_MARKER: AtomicU64 = AtomicU64::new(0);
static ATTEMPTS: AtomicU32 = AtomicU32::new(0);
static STARTUP: Startup = Startup::new();

const SLOT_MASK: u8 = 3;
const WRITING: u8 = 1;
const READY: u8 = 2;
const CLAIMED: u8 = 3;
const ENTERED: u8 = 4;
const CLOSED: u8 = 8;

struct Startup {
    control: AtomicU8,
    nonce: [AtomicU8; 16],
    attempt: AtomicU32,
}

impl Startup {
    const fn new() -> Self {
        Self {
            control: AtomicU8::new(0),
            nonce: [const { AtomicU8::new(0) }; 16],
            attempt: AtomicU32::new(0),
        }
    }

    fn claim_writer(&self) -> bool {
        match self
            .control
            .compare_exchange(0, WRITING, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) => true,
            // 启动只改变 ENTERED 位；最多补一次 CAS，不等待另一 writer/consumer。
            Err(ENTERED) => self
                .control
                .compare_exchange(
                    ENTERED,
                    ENTERED | WRITING,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok(),
            Err(_) => false,
        }
    }

    fn accept(&self, nonce: [u8; 16], attempts: &AtomicU32) -> Option<PendingReply<'_>> {
        if !self.claim_writer() {
            return None;
        }
        let Some(attempt) = reserve_attempt(attempts) else {
            self.release_slot();
            return None;
        };
        self.publish(nonce, attempt)
    }

    fn publish(&self, nonce: [u8; 16], attempt: u32) -> Option<PendingReply<'_>> {
        for (cell, byte) in self.nonce.iter().zip(nonce) {
            cell.store(byte, Ordering::Relaxed);
        }
        self.attempt.store(attempt, Ordering::Relaxed);
        // 唯一 writer 将 WRITING 改成 READY，保持并发发布的 ENTERED/CLOSED 位。
        // 与 enter 的 fetch_or 共用一个修改序：必有一方负责消费，无丢失唤醒窗口。
        let previous = self.control.fetch_xor(WRITING ^ READY, Ordering::AcqRel);
        if previous & CLOSED != 0 {
            self.release_slot();
            return None;
        }
        if previous & ENTERED != 0 {
            self.take()
        } else {
            None
        }
    }

    fn enter(&self) -> Option<PendingReply<'_>> {
        self.control.fetch_or(ENTERED, Ordering::AcqRel);
        self.take()
    }

    fn take(&self) -> Option<PendingReply<'_>> {
        self.control
            .compare_exchange(
                ENTERED | READY,
                ENTERED | CLAIMED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .ok()?;
        // READY 的 release 与唯一 CLAIMED 的 acquire 发布完整 payload。
        Some(PendingReply {
            startup: self,
            nonce: std::array::from_fn(|index| self.nonce[index].load(Ordering::Relaxed)),
            attempt: self.attempt.load(Ordering::Relaxed),
        })
    }

    fn release_slot(&self) {
        self.control.fetch_and(!SLOT_MASK, Ordering::Release);
    }

    fn close(&self) {
        self.control.fetch_or(CLOSED, Ordering::AcqRel);
    }
}

struct PendingReply<'a> {
    startup: &'a Startup,
    nonce: [u8; 16],
    attempt: u32,
}

impl PendingReply<'_> {
    fn can_reply(&self) -> bool {
        // 同一控制字上的发射门 CAS：关闭先线性化则拒绝，已发射者属于 inflight。
        self.startup
            .control
            .compare_exchange(
                ENTERED | CLAIMED,
                ENTERED | CLAIMED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }
}

impl Drop for PendingReply<'_> {
    fn drop(&mut self) {
        // 直到回应函数返回（包括 unwind）才释放；不会覆盖就绪或正在消费的单槽。
        self.startup.release_slot();
    }
}

fn emit_reply(pending: PendingReply<'_>) {
    if pending.can_reply() {
        reply(pending.nonce, pending.attempt);
    }
}

struct StartupLifetime<'a>(&'a Startup);

impl Drop for StartupLifetime<'_> {
    fn drop(&mut self) {
        self.0.close();
    }
}

/// 父级 main 必须使用这个同一谓词，且位于任何 app/lifecycle 启动之前。
pub fn fixture_only_mode() -> bool {
    cfg!(all(
        feature = "native-input-spike",
        target_os = "windows",
        target_pointer_width = "64"
    )) && option_env!("GUOSSH_NATIVE_INPUT_SPIKE_FIXTURE") == Some("1")
}

/// 非分配的启动前查询；1 表示这份受控构建要求 fixture-only main 分支。
#[unsafe(no_mangle)]
pub extern "C" fn guossh_native_input_sdk_fixture_mode() -> u32 {
    u32::from(fixture_only_mode())
}

/// 仅父级 main 的 fixture 分支调用；不调用 app、lifecycle、数据库或钥匙串。
pub async fn run_fixture_only_until_shutdown() {
    if !fixture_only_mode() {
        return;
    }
    let _lifetime = StartupLifetime(&STARTUP);
    let pending = enter_startup();
    if let Some(pending) = pending {
        emit_reply(pending);
    }
    rinf::dart_shutdown().await;
}

fn enter_startup() -> Option<PendingReply<'static>> {
    #[cfg(test)]
    tests::pause_startup_publish();
    STARTUP.enter()
}

fn decode_request(bytes: &[u8; REQUEST_LEN]) -> Option<[u8; 16]> {
    if &bytes[..4] != b"GNSQ"
        || u16::from_le_bytes([bytes[4], bytes[5]]) != VERSION
        || u16::from_le_bytes([bytes[6], bytes[7]]) != 1
        || bytes[8..12] != 1_u32.to_le_bytes()
        || bytes[12..16] != [0; 4]
        || bytes[16..].iter().all(|byte| *byte == 0)
    {
        return None;
    }
    let mut nonce = [0; 16];
    nonce.copy_from_slice(&bytes[16..]);
    Some(nonce)
}

fn reserve_attempt(counter: &AtomicU32) -> Option<u32> {
    // 生产调用由单槽 writer 独占；只做一次 CAS，预算耗尽或竞争均不等待。
    let previous = counter.load(Ordering::Acquire);
    if previous >= MAX_RESPONSE_ATTEMPTS {
        return None;
    }
    counter
        .compare_exchange(previous, previous + 1, Ordering::AcqRel, Ordering::Acquire)
        .ok()
        .map(|previous| previous + 1)
}

fn encode_response(nonce: [u8; 16], marker: u64, attempt: u32) -> [u8; RESPONSE_LEN] {
    let mut bytes = [0; RESPONSE_LEN];
    bytes[..4].copy_from_slice(b"GNSR");
    bytes[4..6].copy_from_slice(&VERSION.to_le_bytes());
    // status=0：只有实际进入 fixture-only 分支才允许回应。
    bytes[8..12].copy_from_slice(&CORE_ABI.to_le_bytes());
    bytes[12..16].copy_from_slice(&(usize::BITS).to_le_bytes());
    bytes[16..32].copy_from_slice(&nonce);
    bytes[32..40].copy_from_slice(&marker.to_le_bytes());
    bytes[40..44].copy_from_slice(&1_u32.to_le_bytes());
    bytes[44..48].copy_from_slice(&attempt.to_le_bytes());
    bytes
}

fn reply(nonce: [u8; 16], attempt: u32) {
    MODULE_MARKER.store(u64::from(attempt), Ordering::Release);
    let marker = (&MODULE_MARKER as *const AtomicU64) as usize as u64;
    let bytes = encode_response(nonce, marker, attempt);
    // SDK 的 Ok 不是 delivered；成功只由 Dart 实收 matching nonce 判定。
    #[cfg(test)]
    tests::capture_reply(REPLY_SIGNAL, &bytes);
    #[cfg(not(test))]
    let _ = rinf::send_rust_signal(REPLY_SIGNAL, bytes.to_vec(), Vec::new());
}

/// Rinf 公共 sendDartSignal 所用的四参 ABI；只读取固定非秘密请求。
///
/// # Safety
/// message 在 message_len=32 时指向有效、可读、调用期间不变的 32-byte 借用。
/// binary 不被解引用；binary_len 必须为 0。不得向本入口传入秘密或真实输入。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn guossh_native_input_sdk_probe(
    message: *const u8,
    message_len: usize,
    _binary: *const u8,
    binary_len: usize,
) {
    let _ = std::panic::catch_unwind(|| {
        if !fixture_only_mode()
            || message_len != REQUEST_LEN
            || binary_len != 0
            || message.is_null()
        {
            return;
        }
        // SAFETY: 仅精确长度且非空的调用进入借用，剩余有效性由 caller 保证。
        let bytes = unsafe { &*message.cast::<[u8; REQUEST_LEN]>() };
        let Some(nonce) = decode_request(bytes) else {
            return;
        };
        if let Some(pending) = STARTUP.accept(nonce, &ATTEMPTS) {
            emit_reply(pending);
        }
    });
}

#[cfg(test)]
mod tests;
