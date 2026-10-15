use super::{
    MAX_RESPONSE_ATTEMPTS, REQUEST_LEN, Startup, decode_request, emit_reply, encode_response,
    enter_startup, fixture_only_mode, guossh_native_input_sdk_fixture_mode,
    guossh_native_input_sdk_probe, reserve_attempt,
};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Barrier, OnceLock};

static PAUSE_STARTUP: AtomicBool = AtomicBool::new(false);
static STARTUP_GATE: OnceLock<(Barrier, Barrier)> = OnceLock::new();
static REPLIES: AtomicU32 = AtomicU32::new(0);
static MATCHED: AtomicBool = AtomicBool::new(true);

pub(super) fn pause_startup_publish() {
    if PAUSE_STARTUP.load(Ordering::Acquire) {
        let (ready, resume) = STARTUP_GATE.get_or_init(|| (Barrier::new(2), Barrier::new(2)));
        ready.wait();
        resume.wait();
    }
}

pub(super) fn capture_reply(_signal: &str, bytes: &[u8; super::RESPONSE_LEN]) {
    MATCHED.fetch_and(bytes[16..32] == [7; 16], Ordering::AcqRel);
    REPLIES.fetch_add(1, Ordering::AcqRel);
}

#[test]
fn first_ffi_probe_survives_startup_publication_contention() {
    if !fixture_only_mode() {
        return;
    }
    let bytes = request();
    assert_eq!(decode_request(&bytes), Some([7; 16]));
    for offset in [0, 4, 6, 8, 12] {
        let mut malformed = bytes;
        malformed[offset] ^= 2;
        // SAFETY: 无效 wire 仍为有效固定借用，必须在占槽和预算前拒绝。
        unsafe {
            guossh_native_input_sdk_probe(malformed.as_ptr(), malformed.len(), std::ptr::null(), 0);
        }
    }
    let mut zero_nonce = bytes;
    zero_nonce[16..].fill(0);
    unsafe {
        guossh_native_input_sdk_probe(zero_nonce.as_ptr(), zero_nonce.len(), std::ptr::null(), 0);
    }
    assert_eq!(super::STARTUP.control.load(Ordering::Acquire), 0);
    assert_eq!(super::ATTEMPTS.load(Ordering::Acquire), 0);
    let (ready, resume) = STARTUP_GATE.get_or_init(|| (Barrier::new(2), Barrier::new(2)));
    PAUSE_STARTUP.store(true, Ordering::Release);
    std::thread::scope(|scope| {
        let starter = scope.spawn(|| {
            if let Some(pending) = enter_startup() {
                emit_reply(pending);
            }
        });
        ready.wait();
        // SAFETY: 固定合法借用贯穿调用；唯一 probe 与真实启动发布区确定性竞争。
        unsafe {
            guossh_native_input_sdk_probe(bytes.as_ptr(), bytes.len(), std::ptr::null(), 0);
        }
        resume.wait();
        assert!(starter.join().is_ok());
    });
    PAUSE_STARTUP.store(false, Ordering::Release);
    let contended = REPLIES.load(Ordering::Acquire);
    // 同一 wire/nonce 无竞争对照，排除 decoder 与 SDK callback 路径。
    unsafe {
        guossh_native_input_sdk_probe(bytes.as_ptr(), bytes.len(), std::ptr::null(), 0);
    }
    let uncontended = REPLIES.load(Ordering::Acquire) - contended;
    assert!(MATCHED.load(Ordering::Acquire));
    assert_eq!((contended, uncontended), (1, 1));
    super::STARTUP.close();
    unsafe {
        guossh_native_input_sdk_probe(bytes.as_ptr(), bytes.len(), std::ptr::null(), 0);
    }
    assert!(enter_startup().is_none());
    assert_eq!(REPLIES.load(Ordering::Acquire), 2);
}

fn request() -> [u8; REQUEST_LEN] {
    let mut bytes = [0; REQUEST_LEN];
    bytes[..4].copy_from_slice(b"GNSQ");
    bytes[4..6].copy_from_slice(&1_u16.to_le_bytes());
    bytes[6..8].copy_from_slice(&1_u16.to_le_bytes());
    bytes[8..12].copy_from_slice(&1_u32.to_le_bytes());
    bytes[16..].fill(7);
    bytes
}

#[test]
fn fixed_request_accepts_only_explicit_valid_nonce() {
    assert_eq!(decode_request(&request()), Some([7; 16]));
    for offset in [0, 4, 6, 8, 12] {
        let mut bytes = request();
        bytes[offset] ^= 2;
        assert_eq!(decode_request(&bytes), None);
    }
    let mut bytes = request();
    bytes[16..].fill(0);
    assert_eq!(decode_request(&bytes), None);
}

#[test]
fn wrong_lengths_and_null_are_rejected_before_borrow() {
    // SAFETY: 非法长度和 null 在解引用前拒绝，不构造无效有效长度借用。
    unsafe {
        guossh_native_input_sdk_probe(std::ptr::null(), REQUEST_LEN, std::ptr::null(), 0);
        guossh_native_input_sdk_probe(std::ptr::dangling(), 31, std::ptr::null(), 0);
        guossh_native_input_sdk_probe(std::ptr::dangling(), 33, std::ptr::null(), 0);
        guossh_native_input_sdk_probe(std::ptr::dangling(), REQUEST_LEN, std::ptr::null(), 1);
    }
}

#[test]
fn response_is_fixed_nonsecret_layout() {
    let response = encode_response([7; 16], 0x1000, 3);
    assert_eq!(&response[..8], b"GNSR\x01\x00\x00\x00");
    assert_eq!(&response[8..12], &2_u32.to_le_bytes());
    assert_eq!(&response[12..16], &usize::BITS.to_le_bytes());
    assert_eq!(&response[16..32], &[7; 16]);
    assert_eq!(&response[32..40], &0x1000_u64.to_le_bytes());
    assert_eq!(&response[40..44], &1_u32.to_le_bytes());
    assert_eq!(&response[44..48], &3_u32.to_le_bytes());
}

#[test]
fn attempt_budget_never_replenishes_or_wraps() {
    let counter = AtomicU32::new(0);
    for attempt in 1..=MAX_RESPONSE_ATTEMPTS {
        assert_eq!(reserve_attempt(&counter), Some(attempt));
    }
    assert_eq!(reserve_attempt(&counter), None);
    assert_eq!(counter.load(Ordering::Acquire), MAX_RESPONSE_ATTEMPTS);
    let exhausted = AtomicU32::new(u32::MAX);
    assert_eq!(reserve_attempt(&exhausted), None);
    assert_eq!(exhausted.load(Ordering::Acquire), u32::MAX);
}

#[test]
fn startup_race_has_one_fixed_pending_slot() {
    let startup = Startup::new();
    let attempts = AtomicU32::new(0);
    assert!(startup.accept([1; 16], &attempts).is_none());
    assert!(startup.accept([2; 16], &attempts).is_none());
    assert_eq!(attempts.load(Ordering::Acquire), 1);
    let Some(first) = startup.enter() else {
        panic!("启动必须取得已发布的单槽回应");
    };
    assert_eq!((first.nonce, first.attempt), ([1; 16], 1));
    assert!(startup.enter().is_none());
    assert!(startup.accept([2; 16], &attempts).is_none());
    drop(first);
    let Some(next) = startup.accept([3; 16], &attempts) else {
        panic!("已进入的空槽必须接收合法请求");
    };
    assert_eq!((next.nonce, next.attempt), ([3; 16], 2));
}

#[test]
fn startup_during_writer_publication_hands_reply_back_to_writer() {
    let startup = Startup::new();
    let ready = Barrier::new(2);
    let resume = Barrier::new(2);
    std::thread::scope(|scope| {
        let writer = scope.spawn(|| {
            assert!(startup.claim_writer());
            ready.wait();
            resume.wait();
            let Some(pending) = startup.publish([5; 16], 1) else {
                panic!("启动已发布时必须由 writer 取得回应");
            };
            assert_eq!((pending.nonce, pending.attempt), ([5; 16], 1));
        });
        ready.wait();
        // writer 持有单槽但尚未 publish；启动发布不能等待或抢读不完整 payload。
        assert!(startup.enter().is_none());
        resume.wait();
        assert!(writer.join().is_ok());
    });
    assert!(startup.take().is_none());
}

#[test]
fn concurrent_startup_consumers_claim_pending_nonce_exactly_once() {
    let startup = Startup::new();
    let attempts = AtomicU32::new(0);
    let ready = Barrier::new(3);
    let claims = AtomicU32::new(0);
    assert!(startup.accept([6; 16], &attempts).is_none());
    std::thread::scope(|scope| {
        for _ in 0..2 {
            scope.spawn(|| {
                ready.wait();
                if let Some(pending) = startup.enter() {
                    assert_eq!((pending.nonce, pending.attempt), ([6; 16], 1));
                    claims.fetch_add(1, Ordering::AcqRel);
                }
            });
        }
        ready.wait();
    });
    assert_eq!(claims.load(Ordering::Acquire), 1);
    assert!(startup.take().is_none());
}

#[test]
fn closed_startup_rejects_ready_writing_and_claimed_handoffs() {
    let attempts = AtomicU32::new(0);
    let ready = Startup::new();
    assert!(ready.accept([1; 16], &attempts).is_none());
    ready.close();
    assert!(ready.enter().is_none());
    assert!(ready.accept([2; 16], &attempts).is_none());

    let writing = Startup::new();
    assert!(writing.claim_writer());
    assert!(writing.enter().is_none());
    writing.close();
    assert!(writing.publish([3; 16], 2).is_none());
    assert!(writing.enter().is_none());

    let claimed = Startup::new();
    assert!(claimed.enter().is_none());
    let Some(pending) = claimed.accept([4; 16], &attempts) else {
        panic!("关闭前必须能取得合法回应");
    };
    claimed.close();
    assert!(!pending.can_reply());
    drop(pending);
    assert!(claimed.enter().is_none());
    assert!(claimed.accept([5; 16], &attempts).is_none());
}

#[test]
fn exhausted_budget_releases_slot_without_reopening_or_wrapping() {
    let startup = Startup::new();
    let attempts = AtomicU32::new(0);
    assert!(startup.enter().is_none());
    for expected in 1..=MAX_RESPONSE_ATTEMPTS {
        let Some(pending) = startup.accept([1; 16], &attempts) else {
            panic!("预算内的空槽必须接收合法请求");
        };
        assert_eq!(pending.attempt, expected);
    }
    assert!(startup.accept([2; 16], &attempts).is_none());
    assert!(startup.accept([3; 16], &attempts).is_none());
    assert_eq!(attempts.load(Ordering::Acquire), MAX_RESPONSE_ATTEMPTS);
    assert_eq!(startup.control.load(Ordering::Acquire), super::ENTERED);
}

#[test]
fn fixture_lifetime_exit_closes_pending_without_resurrection() {
    let startup = Startup::new();
    let attempts = AtomicU32::new(0);
    {
        let _lifetime = super::StartupLifetime(&startup);
        assert!(startup.accept([1; 16], &attempts).is_none());
    }
    assert!(startup.enter().is_none());
    assert!(startup.accept([2; 16], &attempts).is_none());
    assert_eq!(attempts.load(Ordering::Acquire), 1);
}

#[test]
fn mode_requires_feature_and_compile_time_opt_in() {
    let expected = cfg!(all(
        feature = "native-input-spike",
        target_os = "windows",
        target_pointer_width = "64"
    )) && option_env!("GUOSSH_NATIVE_INPUT_SPIKE_FIXTURE") == Some("1");
    assert_eq!(fixture_only_mode(), expected);
    assert_eq!(guossh_native_input_sdk_fixture_mode(), u32::from(expected));
}
